// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux Vulkan color readback into canonical guest GPU memory.

use exosphere_gpu_ir::{ResourceId, TextureFormat, maxwell_state::DrawStateSnapshot};
use exosphere_gpu_vulkan::ash_device::AshVulkanDevice;
use exosphere_horizon::{memory::AddressSpace, nvdrv::address_space::NvGpuAddressSpace};

#[derive(Debug, thiserror::Error)]
pub enum ColorReadbackError {
    #[error("texture payload: {0:?}")]
    TexturePayload(exosphere_gpu_ir::texture_data::TextureDataError),
    #[error("RGBA8 readback requires an RGBA8 guest color target")]
    UnsupportedFormat,
    #[error("Vulkan image dimensions differ from the captured guest target")]
    ShapeMismatch,
    #[error("unsupported single-color draw module")]
    UnsupportedDraw,
    #[error(transparent)]
    Compile(#[from] exosphere_gpu_ir::compiler::GpuCompileError),
    #[error(transparent)]
    Index(#[from] exosphere_runtime::gpu::index_memory::IndexReadError),
    #[error(transparent)]
    ShaderExtent(#[from] exosphere_gpu_ir::shader_extent::ShaderExtentError),
    #[error(transparent)]
    ShaderIdentity(#[from] exosphere_runtime::gpu::shader_identity::ShaderIdentityExhausted),
    #[error(transparent)]
    PipelineIdentity(#[from] exosphere_runtime::gpu::pipeline_identity::PipelineIdentityExhausted),
    #[error(transparent)]
    Resources(#[from] exosphere_runtime::gpu::resource_memory::GuestGpuResourceError),
    #[error(transparent)]
    Mapping(#[from] exosphere_gpu_ir::address_space::GpuAddressError),
    #[error(transparent)]
    Vulkan(#[from] exosphere_gpu_vulkan::VulkanError),
    #[error(transparent)]
    Guest(#[from] exosphere_runtime::gpu::color_memory::ColorWriteError),
}

pub struct ColorDrawTarget<'a> {
    pub snapshot: &'a DrawStateSnapshot,
    pub index: usize,
    pub image: ResourceId,
}

/// Prepared shaders and allocation metadata remain owned by the channel's
/// resource provider. This request borrows them for one immutable draw capture.
pub struct ColorVertexArrayDraw<'a> {
    pub event: &'a exosphere_gpu_ir::MaxwellEvent,
    pub inputs: exosphere_gpu_ir::compiler::ColorVertexArrayInputs<'a>,
    pub address_space: &'a exosphere_gpu_ir::GpuAddressSpace,
}

/// Shader/pipeline namespace owned by one native device, never guest handles.
#[derive(Debug)]
pub struct ColorDrawIdentities {
    shaders: exosphere_runtime::gpu::shader_identity::ShaderIdentityCatalog,
    pipelines: exosphere_runtime::gpu::pipeline_identity::PipelineIdentityCatalog,
}

impl ColorDrawIdentities {
    pub fn new(first_shader: u64, first_pipeline: u64) -> Self {
        Self {
            shaders: exosphere_runtime::gpu::shader_identity::ShaderIdentityCatalog::new(
                first_shader,
            ),
            pipelines: exosphere_runtime::gpu::pipeline_identity::PipelineIdentityCatalog::new(
                first_pipeline,
            ),
        }
    }
}

/// Persistent state for one canonical channel's supported color draw path.
/// Closing its owner drops device caches; this object retains no guest leases.
pub struct GuestColorRenderer {
    device: AshVulkanDevice,
    resources: exosphere_runtime::gpu::resource_memory::GuestGpuResourceCatalog,
    identities: ColorDrawIdentities,
    images: Vec<(
        exosphere_gpu_ir::maxwell_state::ColorTargetState,
        ResourceId,
        u64,
        ResourceId,
    )>,
    next_image: u64,
    sampled_images: Vec<(
        exosphere_gpu_ir::texture_backing::TextureBacking,
        ResourceId,
    )>,
    samplers: Vec<exosphere_gpu_ir::SamplerDesc>,
}

pub struct VerifiedDrawPrograms {
    pub windows: Vec<exosphere_gpu_ir::compiler::ShaderWindow>,
    pub groups: exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups,
}

/// Resolve verified program extents and stage ownership for a captured draw.
/// A mapping's remaining bytes alone are not evidence of a shader's length.
pub trait DrawProgramProvider: std::fmt::Debug {
    fn resolve(
        &mut self,
        identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        state: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError>;

    /// Idempotent cleanup, including channels that never rendered successfully.
    fn close_channel(&mut self, _identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity) {}
}

/// Live, uncached discovery for straight-line shaders and certified direct BRA
/// edges. Stage ownership is explicit; calls and indirect flow still reject.
#[derive(Debug)]
pub struct DirectDrawProgramProvider {
    maximum_shader_bytes: usize,
    groups: exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups,
}

impl DirectDrawProgramProvider {
    pub fn new(
        maximum_shader_bytes: usize,
        groups: exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups,
    ) -> Result<Self, ColorReadbackError> {
        if !(96..=64 * 1024 * 1024).contains(&maximum_shader_bytes) {
            return Err(exosphere_gpu_ir::shader_extent::ShaderExtentError::InvalidBudget.into());
        }
        let group_count = exosphere_gpu_ir::maxwell_state::method::CONSTANT_BUFFER_GROUP_COUNT;
        if usize::from(groups.vertex_group) >= group_count
            || usize::from(groups.fragment_group) >= group_count
        {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
        Ok(Self {
            maximum_shader_bytes,
            groups,
        })
    }

    fn resolve_programs(
        &self,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        state: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError> {
        let programs = exosphere_gpu_ir::shader_discovery::discover_draw_shaders(state)
            .map_err(exosphere_gpu_ir::compiler::GpuCompileError::Memory)?;
        // Admit the supported complete stage pair before touching shader memory
        // or creating Vulkan state. Geometry/tessellation and dual vertex stages
        // need their own preparation path, not a partially read draw.
        if programs.len() != 2
            || programs
                .iter()
                .filter(|program| program.stage == exosphere_gpu_ir::ShaderStage::Vertex)
                .count()
                != 1
            || programs
                .iter()
                .filter(|program| program.stage == exosphere_gpu_ir::ShaderStage::Fragment)
                .count()
                != 1
        {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
        let memory = exosphere_runtime::gpu::shader_memory::GuestShaderMemory { memory, gpu };
        let mut windows = Vec::new();
        for program in &programs {
            // The current independent-program path must not borrow termination
            // from another captured entry point. Shared code tails need explicit
            // ownership/CFG support rather than concatenating adjacent programs.
            let maximum_bytes = programs
                .iter()
                .filter(|other| other.gpu_address > program.gpu_address)
                .map(|other| other.gpu_address - program.gpu_address)
                .min()
                .unwrap_or(self.maximum_shader_bytes as u64)
                .min(self.maximum_shader_bytes as u64) as usize;
            windows.push(
                exosphere_gpu_ir::shader_extent::discover_direct_graphics_window(
                    &memory,
                    program.gpu_address,
                    maximum_bytes,
                )?,
            );
        }
        Ok(VerifiedDrawPrograms {
            windows,
            groups: self.groups,
        })
    }
}

impl DrawProgramProvider for DirectDrawProgramProvider {
    fn resolve(
        &mut self,
        _: &exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        state: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError> {
        // Re-read on every draw: shader bytes can change at the same GPU address.
        self.resolve_programs(memory, gpu, state)
    }
}

/// Installable driver renderer with isolated native caches per canonical channel.
#[derive(Debug)]
pub struct LinuxDrawRenderer<P: DrawProgramProvider> {
    provider: P,
    channels: Vec<(
        exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        GuestColorRenderer,
    )>,
    maximum_channels: usize,
}

impl<P: DrawProgramProvider> LinuxDrawRenderer<P> {
    pub fn new(provider: P, maximum_channels: usize) -> Result<Self, ColorReadbackError> {
        if maximum_channels == 0 || maximum_channels > 4096 {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
        Ok(Self {
            provider,
            channels: Vec::new(),
            maximum_channels,
        })
    }
}

impl<P: DrawProgramProvider> exosphere_runtime::gpu::nv_backend::NativeDrawRenderer
    for LinuxDrawRenderer<P>
{
    fn draw(
        &mut self,
        identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        event: &exosphere_gpu_ir::MaxwellEvent,
    ) -> Result<(), exosphere_runtime::gpu::GuestGpuExecutionFailure> {
        let mut execute = || -> Result<(), ColorReadbackError> {
            let state = match event {
                exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. }
                | exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } => state,
                _ => return Err(ColorReadbackError::UnsupportedDraw),
            };
            let programs = self.provider.resolve(identity, memory, gpu, state)?;
            let index = match self
                .channels
                .iter()
                .position(|(owner, _)| owner == identity)
            {
                Some(index) => index,
                None => {
                    if self.channels.len() == self.maximum_channels {
                        return Err(ColorReadbackError::UnsupportedDraw);
                    }
                    let renderer = GuestColorRenderer::create()?;
                    self.channels.push((identity.clone(), renderer));
                    self.channels.len() - 1
                }
            };
            self.channels[index]
                .1
                .draw(memory, gpu, event, &programs.windows, &programs.groups)
        };
        execute().map_err(|error| {
            exosphere_runtime::gpu::GuestGpuExecutionFailure::Draw(Box::new(error))
        })
    }

    fn close_channel(&mut self, identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity) {
        self.channels.retain(|(owner, _)| owner != identity);
        self.provider.close_channel(identity);
    }
}

impl std::fmt::Debug for GuestColorRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestColorRenderer")
            .field("images", &self.images.len())
            .field("sampled_images", &self.sampled_images.len())
            .field("samplers", &self.samplers.len())
            .finish_non_exhaustive()
    }
}

impl GuestColorRenderer {
    pub fn create() -> Result<Self, ColorReadbackError> {
        Ok(Self {
            device: AshVulkanDevice::create().map_err(exosphere_gpu_vulkan::VulkanError::Driver)?,
            resources: exosphere_runtime::gpu::resource_memory::GuestGpuResourceCatalog::new(1),
            identities: ColorDrawIdentities::new(1, 1),
            images: Vec::new(),
            next_image: u64::MAX,
            sampled_images: Vec::new(),
            samplers: Vec::new(),
        })
    }

    fn reserve_view_id(
        &mut self,
        resources: &exosphere_runtime::gpu::resource_memory::GuestGpuResourceSnapshot,
    ) -> Result<ResourceId, exosphere_gpu_ir::compiler::GpuCompileError> {
        let id = ResourceId(self.next_image);
        if resources
            .allocations
            .iter()
            .any(|allocation| allocation.id.0 >= id.0)
        {
            return Err(exosphere_gpu_ir::compiler::GpuCompileError::InvalidDrawState);
        }
        self.next_image = self
            .next_image
            .checked_sub(1)
            .ok_or(exosphere_gpu_ir::compiler::GpuCompileError::InvalidDrawState)?;
        Ok(id)
    }

    fn prepare_texture(
        &mut self,
        records: &exosphere_gpu_ir::tic_tsc::TextureHandleRecords,
        resources: &exosphere_runtime::gpu::resource_memory::GuestGpuResourceSnapshot,
    ) -> Result<
        exosphere_gpu_ir::texture_sampling::PreparedTexture2d,
        exosphere_gpu_ir::compiler::GpuCompileError,
    > {
        use exosphere_gpu_ir::compiler::GpuCompileError as E;
        let backing = records
            .resolve_image_backing(&resources.address_space)
            .map_err(|error| {
                E::TextureSampling(
                    exosphere_gpu_ir::texture_sampling::TextureSamplingError::Backing(error),
                )
            })?;
        let image = match self
            .sampled_images
            .iter()
            .find(|(known, _)| *known == backing)
        {
            Some((_, id)) => *id,
            None => {
                let id = self.reserve_view_id(resources)?;
                self.sampled_images.push((backing, id));
                id
            }
        };
        // Only consume a sampler identity if its complete canonical state is
        // new. Pixel refresh does not change either native resource identity.
        let candidate = ResourceId(self.next_image);
        let mut view = records
            .prepare_sampling(image, candidate, &resources.address_space)
            .map_err(E::TextureSampling)?;
        match self
            .samplers
            .iter()
            .find(|sampler| sampler.state == view.sampler.state)
        {
            Some(sampler) => view.sampler.id = sampler.id,
            None => {
                view.sampler.id = self.reserve_view_id(resources)?;
                self.samplers.push(view.sampler);
            }
        }
        Ok(view)
    }

    /// Windows and stage ownership must be verified by the caller. Interfaces,
    /// allocation roles and cache identities are derived from actual draw data.
    pub fn draw(
        &mut self,
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        event: &exosphere_gpu_ir::MaxwellEvent,
        windows: &[exosphere_gpu_ir::compiler::ShaderWindow],
        groups: &exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups,
    ) -> Result<(), ColorReadbackError> {
        let state = match event {
            exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. }
            | exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } => state,
            _ => return Err(ColorReadbackError::UnsupportedDraw),
        };
        let resources = self.resources.refresh(memory, gpu)?;
        if resources
            .allocations
            .iter()
            .any(|allocation| allocation.id.0 >= self.next_image)
        {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
        let direct = exosphere_runtime::gpu::shader_memory::prepare_textured_event_draw_shaders(
            memory,
            gpu,
            event,
            windows,
            groups,
            &resources.address_space,
            |records| self.prepare_texture(records, &resources),
        )?;
        let indexed = if matches!(
            event,
            exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { .. }
        ) {
            Some(
                exosphere_runtime::gpu::index_memory::prepare_captured_indexed_vertex_inputs(
                    memory,
                    gpu,
                    event,
                    &direct.attributes,
                    &resources.address_space,
                )?,
            )
        } else {
            None
        };
        let mut allocations = match &indexed {
            Some(indexed) => {
                resources.prepare_indexed_draw_allocations(indexed, &direct.shaders)?
            }
            None => {
                resources.prepare_draw_allocations(event, &direct.shaders, &direct.attributes)?
            }
        };
        // Canonical allocations can gain another input role on a later draw.
        // Native buffers cannot gain Vulkan usage flags after creation, so use
        // one stable capability superset from their first submission. This does
        // not fabricate guest bindings or change allocation identity/size.
        for allocation in &mut allocations {
            allocation.usage = exosphere_gpu_ir::resource::BufferUsage::IndexVertexUniform;
        }
        let target = state
            .color_target(0)
            .ok_or(ColorReadbackError::UnsupportedDraw)?;
        let (address, size) = target
            .block_linear_layer_range()
            .ok_or(ColorReadbackError::UnsupportedDraw)?;
        let backing = resources.address_space.resolve(address, size)?;
        let image = match self.images.iter().find(|(known, resource, offset, _)| {
            *known == target && *resource == backing.resource && *offset == backing.offset
        }) {
            Some((_, _, _, image)) => *image,
            None => {
                let image = self.reserve_view_id(&resources)?;
                self.images
                    .push((target, backing.resource, backing.offset, image));
                image
            }
        };
        // Color images and buffers share a module namespace. Never silently
        // reuse a buffer identity if the opposing ID sequences meet.
        if resources
            .allocations
            .iter()
            .any(|allocation| allocation.id == image)
        {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
        let request = ColorVertexArrayDraw {
            event,
            inputs: exosphere_gpu_ir::compiler::ColorVertexArrayInputs {
                pipeline_id: 0,
                image,
                vertex: &direct.shaders.vertex,
                fragment: &direct.shaders.fragment,
                attributes: &direct.attributes,
                allocations: &allocations,
            },
            address_space: &resources.address_space,
        };
        let indexed_draw = match &indexed {
            Some(indexed) => {
                let mut bindings = indexed.draw_commands()?;
                let Some(range) = indexed.index_range else {
                    return Ok(());
                };
                let (vertex_input, _) =
                    exosphere_gpu_ir::maxwell_state::merge_vertex_inputs(&indexed.inputs)
                        .map_err(exosphere_runtime::gpu::index_memory::IndexReadError::from)?;
                let draw = bindings.pop().ok_or(ColorReadbackError::UnsupportedDraw)?;
                Some((
                    exosphere_gpu_ir::pipeline::PreparedVertexArrayDraw {
                        topology: exosphere_gpu_ir::pipeline::draw_topology(state)
                            .ok_or(ColorReadbackError::UnsupportedDraw)?,
                        vertex_input,
                        bindings,
                        draw,
                        vertex_ranges: indexed.inputs.iter().map(|input| input.range).collect(),
                    },
                    range,
                ))
            }
            None => None,
        };
        let module = prepare_identified_color_draw(&request, &mut self.identities, indexed_draw)?;
        execute_color_draw_from_guest_with_textures(
            &mut self.device,
            memory,
            gpu,
            ColorDrawTarget {
                snapshot: state,
                index: 0,
                image,
            },
            &module,
            &resources.address_space,
            &direct.shaders.fragment.textures,
        )
    }
}

/// Assemble with immutable content/state identities, ignoring provisional IDs
/// from the caller. Original prepared shader provenance is left untouched.
pub fn prepare_identified_color_vertex_array_draw(
    draw: &ColorVertexArrayDraw<'_>,
    identities: &mut ColorDrawIdentities,
) -> Result<exosphere_gpu_ir::GpuModule, ColorReadbackError> {
    prepare_identified_color_draw(draw, identities, None)
}

fn prepare_identified_color_draw(
    draw: &ColorVertexArrayDraw<'_>,
    identities: &mut ColorDrawIdentities,
    indexed: Option<(
        exosphere_gpu_ir::pipeline::PreparedVertexArrayDraw,
        exosphere_gpu_ir::address_space::ResolvedGpuRange,
    )>,
) -> Result<exosphere_gpu_ir::GpuModule, ColorReadbackError> {
    let mut vertex = draw.inputs.vertex.clone();
    let mut fragment = draw.inputs.fragment.clone();
    for shader in [&mut vertex, &mut fragment] {
        let code = exosphere_gpu_ir::spirv::compile(&shader.shader.module)
            .map_err(exosphere_gpu_ir::compiler::GpuCompileError::Interface)?;
        shader.shader.module.id = identities
            .shaders
            .id_for(shader.shader.module.stage, &code.words)?;
    }
    let inputs = exosphere_gpu_ir::compiler::ColorVertexArrayInputs {
        pipeline_id: 0,
        image: draw.inputs.image,
        vertex: &vertex,
        fragment: &fragment,
        attributes: draw.inputs.attributes,
        allocations: draw.inputs.allocations,
    };
    let mut module = match indexed {
        Some((indexed, range)) => {
            let exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } = draw.event else {
                return Err(ColorReadbackError::UnsupportedDraw);
            };
            exosphere_gpu_ir::compiler::prepare_color_indexed_module(
                state,
                &inputs,
                draw.address_space,
                indexed,
                range,
            )?
        }
        None => exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
            draw.event,
            &inputs,
            draw.address_space,
        )?,
    };
    let pipeline = module
        .graphics_pipelines
        .first_mut()
        .ok_or(ColorReadbackError::UnsupportedDraw)?;
    pipeline.id = identities.pipelines.id_for(pipeline)?;
    for command in &mut module.commands {
        if let exosphere_gpu_ir::Command::BindGraphicsPipeline { pipeline: id } = command {
            *id = pipeline.id;
        }
    }
    Ok(module)
}

pub fn execute_identified_color_vertex_array_draw_from_guest(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    draw: ColorVertexArrayDraw<'_>,
    identities: &mut ColorDrawIdentities,
) -> Result<(), ColorReadbackError> {
    let module = prepare_identified_color_vertex_array_draw(&draw, identities)?;
    let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. } = draw.event else {
        return Err(ColorReadbackError::UnsupportedDraw);
    };
    execute_color_draw_from_guest(
        device,
        memory,
        gpu,
        ColorDrawTarget {
            snapshot: state,
            index: 0,
            image: draw.inputs.image,
        },
        &module,
        draw.address_space,
    )
}

/// Assemble and synchronously execute the supported guest vertex-array path.
/// Shader interfaces and resource identities must come from the caller's live
/// channel resources; this function invents neither shader code nor clear pixels.
pub fn execute_color_vertex_array_draw(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    draw: ColorVertexArrayDraw<'_>,
    resources: &mut dyn exosphere_gpu_ir::GpuResourceData,
) -> Result<(), ColorReadbackError> {
    let module = exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
        draw.event,
        &draw.inputs,
        draw.address_space,
    )?;
    let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. } = draw.event else {
        return Err(ColorReadbackError::UnsupportedDraw);
    };
    execute_color_draw(
        device,
        memory,
        gpu,
        ColorDrawTarget {
            snapshot: state,
            index: 0,
            image: draw.inputs.image,
        },
        &module,
        resources,
    )
}

/// Assemble a draw and fetch its payloads through live canonical guest mappings.
/// `draw.address_space` must use the allocation identities in `draw.inputs`;
/// descriptor ranges do not substitute for actual allocation sizes.
pub fn execute_color_vertex_array_draw_from_guest(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    draw: ColorVertexArrayDraw<'_>,
) -> Result<(), ColorReadbackError> {
    let module = exosphere_gpu_ir::compiler::prepare_color_vertex_array_module(
        draw.event,
        &draw.inputs,
        draw.address_space,
    )?;
    let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. } = draw.event else {
        return Err(ColorReadbackError::UnsupportedDraw);
    };
    execute_color_draw_from_guest(
        device,
        memory,
        gpu,
        ColorDrawTarget {
            snapshot: state,
            index: 0,
            image: draw.inputs.image,
        },
        &module,
        draw.address_space,
    )
}

/// Execute a supported single-color module with guest attachment load/writeback.
/// A failure after device execution is not permission to replay the whole draw.
/// This entry point does not translate Maxwell commands or signal guest fences.
pub fn execute_color_draw(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    target: ColorDrawTarget<'_>,
    module: &exosphere_gpu_ir::GpuModule,
    resources: &mut dyn exosphere_gpu_ir::GpuResourceData,
) -> Result<(), ColorReadbackError> {
    use exosphere_gpu_vulkan::device::VulkanDevice;
    execute_color_draw_with_resources(device, memory, gpu, target, module, |device, _, _, plan| {
        device.execute_plan_with_resources(plan, resources)?;
        Ok(())
    })
}

/// Read vertex/uniform payloads from the channel's current guest memory during
/// execution, releasing the provider borrow before canonical color writeback.
pub fn execute_color_draw_from_guest(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    target: ColorDrawTarget<'_>,
    module: &exosphere_gpu_ir::GpuModule,
    address_space: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<(), ColorReadbackError> {
    execute_color_draw_from_guest_with_textures(
        device,
        memory,
        gpu,
        target,
        module,
        address_space,
        &[],
    )
}

pub fn execute_color_draw_from_guest_with_textures(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    target: ColorDrawTarget<'_>,
    module: &exosphere_gpu_ir::GpuModule,
    address_space: &exosphere_gpu_ir::GpuAddressSpace,
    textures: &[exosphere_gpu_ir::texture_sampling::PreparedTexture2d],
) -> Result<(), ColorReadbackError> {
    use exosphere_gpu_vulkan::device::VulkanDevice;
    for image in module
        .textures
        .iter()
        .filter(|image| image.id != target.image)
    {
        if !textures.iter().any(|view| view.image == *image) {
            return Err(ColorReadbackError::UnsupportedDraw);
        }
    }
    execute_color_draw_with_resources(
        device,
        memory,
        gpu,
        target,
        module,
        |device, memory, gpu, plan| {
            let mut bridge =
                exosphere_runtime::gpu::shader_memory::GuestGpuResourceMemory { memory, gpu };
            let mut resources =
                exosphere_gpu_ir::MappedGpuResourceData::new(address_space, &mut bridge);
            let mut provider =
                exosphere_gpu_ir::texture_data::TextureResourceData::new(&mut resources);
            let mut registered = std::collections::BTreeMap::new();
            for texture in textures {
                if let Some(previous) = registered.insert(texture.image.id, &texture.backing) {
                    if previous != &texture.backing {
                        return Err(ColorReadbackError::UnsupportedDraw);
                    }
                    continue;
                }
                let current = provider
                    .add_texture(
                        texture.image.id,
                        texture.backing.descriptor.raw.words,
                        address_space,
                    )
                    .map_err(ColorReadbackError::TexturePayload)?;
                if current != texture.backing {
                    return Err(ColorReadbackError::UnsupportedDraw);
                }
            }
            device.execute_plan_with_resources(plan, &mut provider)?;
            Ok(())
        },
    )
}

fn execute_color_draw_with_resources(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    target: ColorDrawTarget<'_>,
    module: &exosphere_gpu_ir::GpuModule,
    execute: impl FnOnce(
        &mut AshVulkanDevice,
        &mut AddressSpace,
        &NvGpuAddressSpace,
        &exosphere_gpu_vulkan::plan::VulkanExecutionPlan,
    ) -> Result<(), ColorReadbackError>,
) -> Result<(), ColorReadbackError> {
    use exosphere_gpu_ir::Command;
    let selection = exosphere_gpu_ir::pipeline::captured_color_targets(target.snapshot)
        .ok_or(ColorReadbackError::UnsupportedDraw)?;
    if selection.targets.as_slice() != [target.index as u8] || target.index >= 8 {
        return Err(ColorReadbackError::UnsupportedDraw);
    }
    let descriptor = target
        .snapshot
        .color_target(target.index)
        .ok_or(ColorReadbackError::UnsupportedFormat)?;
    let mut attachments = module
        .textures
        .iter()
        .filter(|image| image.id == target.image);
    let attachment = attachments
        .next()
        .ok_or(ColorReadbackError::UnsupportedDraw)?;
    if attachments.next().is_some()
        || attachment.format != TextureFormat::Rgba8Unorm
        || attachment.width != descriptor.width
        || attachment.height != descriptor.height
        || attachment.depth != 1
        || attachment.layers != 1
        || attachment.mip_levels != 1
        || attachment.samples != 1
    {
        return Err(ColorReadbackError::UnsupportedDraw);
    }
    let mut has_target = false;
    let mut has_draw = false;
    for command in &module.commands {
        match command {
            Command::SetRenderTarget { slot: 0, texture } if *texture == target.image => {
                has_target = true
            }
            Command::Draw { .. } | Command::DrawIndexed { .. } | Command::DrawIndirect { .. } => {
                has_draw = true
            }
            Command::ClearColor { target: image, .. } if *image == target.image => {}
            Command::BindGraphicsPipeline { pipeline } => {
                let descriptor = module
                    .graphics_pipelines
                    .iter()
                    .find(|desc| desc.id == *pipeline)
                    .ok_or(ColorReadbackError::UnsupportedDraw)?;
                let depth = descriptor.fixed_function.depth_stencil;
                // This entry point owns only a single color attachment. Active
                // depth/stencil state cannot be silently rendered without its
                // guest attachment and corresponding load/writeback ownership.
                if depth.depth_test
                    || depth.depth_write
                    || depth.stencil_test
                    || descriptor.fixed_function.multisample.raster_samples != 1
                    || descriptor.layout.descriptors.iter().any(|binding| {
                        !matches!(
                            binding.descriptor_type,
                            exosphere_gpu_ir::DescriptorType::UniformBuffer
                                | exosphere_gpu_ir::DescriptorType::SampledImage
                                | exosphere_gpu_ir::DescriptorType::Sampler
                        )
                    })
                {
                    return Err(ColorReadbackError::UnsupportedDraw);
                }
            }
            Command::BindImageDescriptor { texture, .. }
                if *texture != target.image
                    && module.textures.iter().any(|image| image.id == *texture) => {}
            Command::BindSamplerDescriptor { sampler, .. }
                if module
                    .samplers
                    .iter()
                    .any(|descriptor| descriptor.id == *sampler) => {}
            Command::BeginRenderPass { .. }
            | Command::EndRenderPass
            | Command::BindVertexBuffer { .. }
            | Command::BindIndexBuffer { .. }
            | Command::BindBufferDescriptor { .. }
            | Command::SetViewport { .. }
            | Command::SetScissor { .. } => {}
            _ => return Err(ColorReadbackError::UnsupportedDraw),
        }
    }
    if !has_target || !has_draw {
        return Err(ColorReadbackError::UnsupportedDraw);
    }
    let (address, bytes) = descriptor
        .block_linear_layer_range()
        .ok_or(exosphere_runtime::gpu::color_memory::ColorWriteError::InvalidDescriptor)?;
    let size = usize::try_from(bytes)
        .map_err(|_| exosphere_runtime::gpu::color_memory::ColorWriteError::Budget)?;
    gpu.validate_access(
        memory,
        address,
        size,
        exosphere_horizon::memory::MemoryPermission::READ_WRITE,
    )
    .map_err(exosphere_runtime::gpu::color_memory::ColorWriteError::Memory)?;
    let plan = exosphere_gpu_vulkan::VulkanBackend::new().plan(module)?;
    create_color_layer(
        device,
        memory,
        gpu,
        target.snapshot,
        target.index,
        target.image,
    )?;
    execute(device, memory, gpu, &plan)?;
    readback_color_layer(
        device,
        memory,
        gpu,
        target.snapshot,
        target.index,
        target.image,
    )
}

/// Read completed Vulkan pixels and write the selected captured guest layer.
/// The caller owns image identity and submission order; this signals no fence.
pub fn readback_color_layer(
    device: &mut AshVulkanDevice,
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    snapshot: &DrawStateSnapshot,
    index: usize,
    image: ResourceId,
) -> Result<(), ColorReadbackError> {
    if snapshot
        .color_target(index)
        .and_then(|target| target.canonical_format())
        != Some(TextureFormat::Rgba8Unorm)
    {
        return Err(ColorReadbackError::UnsupportedFormat);
    }
    let target = snapshot
        .color_target(index)
        .ok_or(ColorReadbackError::UnsupportedFormat)?;
    if device.rgba8_readback_extent(image)? != (target.width, target.height) {
        return Err(ColorReadbackError::ShapeMismatch);
    }
    let pixels = device.readback_rgba8(image)?;
    exosphere_runtime::gpu::color_memory::write_color_layer(
        memory,
        gpu,
        snapshot,
        index,
        TextureFormat::Rgba8Unorm,
        &pixels,
    )?;
    Ok(())
}

/// Refresh an existing completed Vulkan target from live guest pixels before
/// attachment LOAD. The caller owns draw ordering and image allocation.
pub fn upload_color_layer(
    device: &mut AshVulkanDevice,
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    snapshot: &DrawStateSnapshot,
    index: usize,
    image: ResourceId,
) -> Result<(), ColorReadbackError> {
    let target = snapshot
        .color_target(index)
        .ok_or(ColorReadbackError::UnsupportedFormat)?;
    if target.canonical_format() != Some(TextureFormat::Rgba8Unorm) {
        return Err(ColorReadbackError::UnsupportedFormat);
    }
    if device.rgba8_readback_extent(image)? != (target.width, target.height) {
        return Err(ColorReadbackError::ShapeMismatch);
    }
    let pixels = exosphere_runtime::gpu::color_memory::read_color_layer(
        memory,
        gpu,
        snapshot,
        index,
        TextureFormat::Rgba8Unorm,
    )?;
    device.upload_rgba8(image, &pixels)?;
    Ok(())
}

/// Allocate an RGBA8 attachment from existing guest pixels, without fabricating
/// an initial clear or executing a host substitute draw.
pub fn create_color_layer(
    device: &mut AshVulkanDevice,
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    snapshot: &DrawStateSnapshot,
    index: usize,
    image: ResourceId,
) -> Result<(), ColorReadbackError> {
    let target = snapshot
        .color_target(index)
        .ok_or(ColorReadbackError::UnsupportedFormat)?;
    let pixels = exosphere_runtime::gpu::color_memory::read_color_layer(
        memory,
        gpu,
        snapshot,
        index,
        TextureFormat::Rgba8Unorm,
    )?;
    device.initialize_rgba8(
        &exosphere_gpu_ir::TextureDesc {
            id: image,
            width: target.width,
            height: target.height,
            depth: 1,
            layers: 1,
            mip_levels: 1,
            samples: 1,
            format: TextureFormat::Rgba8Unorm,
            layout: exosphere_gpu_ir::TextureLayout::Linear,
        },
        &pixels,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    mod authenticated_ipc;
    use super::*;
    use exosphere_gpu_ir::{GpuModule, InMemoryGpuResources};
    use exosphere_gpu_vulkan::{VulkanBackend, device::VulkanDevice};
    use exosphere_horizon::{
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn direct_provider_admits_stage_pair_and_groups_before_memory_reads() {
        use exosphere_gpu_ir::maxwell_state::method;
        use exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups;
        let groups = DirectDrawShaderGroups {
            vertex_group: 0,
            fragment_group: 4,
            vertex_set: 0,
            fragment_set: 1,
        };
        for invalid in [
            DirectDrawShaderGroups {
                vertex_group: 5,
                ..groups
            },
            DirectDrawShaderGroups {
                fragment_group: u8::MAX,
                ..groups
            },
        ] {
            assert!(matches!(
                DirectDrawProgramProvider::new(4096, invalid),
                Err(ColorReadbackError::UnsupportedDraw)
            ));
        }
        let provider = DirectDrawProgramProvider::new(4096, groups).unwrap();
        let memory = AddressSpace::default();
        let gpu = NvGpuAddressSpace::default();
        for selections in [
            vec![],
            vec![0x11],
            vec![0x51],
            vec![0x11, 0x11],
            vec![0x11, 0x41],
            vec![0x11, 0x51, 0x21],
        ] {
            let mut state = DrawStateSnapshot::default();
            for (slot, select) in selections.into_iter().enumerate() {
                state.raw_registers.insert(
                    method::PIPELINE_BASE + slot as u16 * method::PIPELINE_STRIDE,
                    select,
                );
            }
            assert!(matches!(
                provider.resolve_programs(&memory, &gpu, &state),
                Err(ColorReadbackError::UnsupportedDraw)
            ));
        }
        let mut state = DrawStateSnapshot::default();
        state.raw_registers.insert(method::PIPELINE_BASE, 0x11);
        state
            .raw_registers
            .insert(method::PIPELINE_BASE + method::PIPELINE_STRIDE, 0x51);
        assert!(matches!(
            provider.resolve_programs(&memory, &gpu, &state),
            Err(ColorReadbackError::ShaderExtent(
                exosphere_gpu_ir::shader_extent::ShaderExtentError::Memory(_)
            ))
        ));
    }

    #[test]
    fn direct_provider_does_not_borrow_an_adjacent_programs_exit() {
        use exosphere_gpu_ir::maxwell_state::method;
        use exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups;
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        let mut state = DrawStateSnapshot::default();
        state
            .raw_registers
            .insert(method::SET_PROGRAM_REGION_B, 0x100000);
        state.raw_registers.insert(method::PIPELINE_BASE, 0x11);
        state
            .raw_registers
            .insert(method::PIPELINE_BASE + method::PIPELINE_STRIDE, 0x51);
        state
            .raw_registers
            .insert(method::PIPELINE_BASE + method::PIPELINE_STRIDE + 1, 96);
        memory
            .write(0x4000 + 96 + 88, &0xe30000000007000fu64.to_le_bytes())
            .unwrap();
        let provider = DirectDrawProgramProvider::new(
            4096,
            DirectDrawShaderGroups {
                vertex_group: 0,
                fragment_group: 4,
                vertex_set: 0,
                fragment_set: 1,
            },
        )
        .unwrap();
        assert!(matches!(
            provider.resolve_programs(&memory, &gpu, &state),
            Err(ColorReadbackError::ShaderExtent(
                exosphere_gpu_ir::shader_extent::ShaderExtentError::MissingTermination
            ))
        ));
        memory
            .write(0x4000 + 88, &0xe30000000007000fu64.to_le_bytes())
            .unwrap();
        let programs = provider.resolve_programs(&memory, &gpu, &state).unwrap();
        assert_eq!(
            programs
                .windows
                .iter()
                .map(|window| window.bytes)
                .collect::<Vec<_>>(),
            vec![96, 96]
        );
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_vertex_and_uniform_fetch_observe_live_canonical_guest_payload() {
        use exosphere_gpu_ir::maxwell_state::method;
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, 2 * PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, 2 * PAGE_SIZE, 0, &backing).unwrap();
        // Uniforms and vertex data share the same real allocation through
        // another GPU alias; they must not manufacture distinct host buffers.
        gpu.map_fixed(0x200000, PAGE_SIZE, PAGE_SIZE, &backing)
            .unwrap();
        let mut renderer = GuestColorRenderer::create().unwrap();
        verify_vertex_uniform_fixture(&mut memory, &gpu, 0x4000, |memory, gpu, event, programs| {
            renderer
                .draw(memory, gpu, event, &programs.windows, &programs.groups)
                .unwrap();
            assert_eq!(renderer.images.len(), 1);
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. } = event else {
                unreachable!()
            };
            let mut state = state.as_ref().clone();
            state.index_buffer.address =
                exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200400 };
            state.index_buffer.limit =
                exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200405 };
            state.index_buffer.index_size_raw = 1;
            state.index_buffer.first = 0;
            state
                .raw_registers
                .insert(method::SET_VERTEX_ELEMENT_BASE, 0);
            state.raw_registers.insert(method::SET_PRIMITIVE_RESTART, 0);
            // Start from the same canonical background, not the previous result.
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &state,
                0,
                TextureFormat::Rgba8Unorm,
                &([17, 43, 91, 255].repeat(32 * 32)),
            )
            .unwrap();
            let indexed_event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(state),
            };
            renderer
                .draw(
                    memory,
                    gpu,
                    &indexed_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            let indexed_pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(indexed_pixels, pixels);
            memory.write(0x5400, &[1, 0, 1, 0, 1, 0]).unwrap();
            let exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } = &indexed_event
            else {
                unreachable!()
            };
            let background = [17, 43, 91, 255].repeat(32 * 32);
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                state,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            renderer
                .draw(
                    memory,
                    gpu,
                    &indexed_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            assert_eq!(
                renderer
                    .device
                    .readback_rgba8(renderer.images[0].3)
                    .unwrap(),
                background
            );
            memory.write(0x5400, &[0, 0, 1, 0, 2, 0]).unwrap();
            renderer
                .draw(
                    memory,
                    gpu,
                    &indexed_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            assert_eq!(
                renderer
                    .device
                    .readback_rgba8(renderer.images[0].3)
                    .unwrap(),
                indexed_pixels
            );
            let mut instance_state = state.as_ref().clone();
            instance_state
                .raw_registers
                .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE, 1);
            instance_state
                .raw_registers
                .insert(method::VERTEX_STREAM_BASE + 3, 3);
            let instance_event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(instance_state),
            };
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                state,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            renderer
                .draw(
                    memory,
                    gpu,
                    &instance_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            assert_eq!(
                renderer
                    .device
                    .readback_rgba8(renderer.images[0].3)
                    .unwrap(),
                background
            );
            renderer
                .draw(
                    memory,
                    gpu,
                    &indexed_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            assert_eq!(
                renderer
                    .device
                    .readback_rgba8(renderer.images[0].3)
                    .unwrap(),
                indexed_pixels
            );
            verify_instance_depth_selection(memory, gpu, state, programs, &mut renderer);
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                state,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            renderer
                .draw(
                    memory,
                    gpu,
                    &indexed_event,
                    &programs.windows,
                    &programs.groups,
                )
                .unwrap();
            assert_eq!(
                renderer
                    .device
                    .readback_rgba8(renderer.images[0].3)
                    .unwrap(),
                indexed_pixels
            );
            indexed_pixels
        });
    }

    fn verify_instance_depth_selection(
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        state: &DrawStateSnapshot,
        programs: &VerifiedDrawPrograms,
        renderer: &mut GuestColorRenderer,
    ) {
        use exosphere_gpu_ir::maxwell_state::method;
        let mut saved_vertices = [0; 48];
        let mut saved_instruction = [0; 8];
        let mut saved_w_instruction = [0; 8];
        memory.read(0x5080, &mut saved_vertices).unwrap();
        memory.read(0x5160, &mut saved_instruction).unwrap();
        memory.read(0x5168, &mut saved_w_instruction).unwrap();
        let positions = [
            [-0.5f32, -0.5, 0.0, 1.0],
            [0.5, -0.5, 0.0, 1.0],
            [0.0, 0.5, 0.0, 1.0],
        ];
        let vertices: Vec<_> = positions
            .into_iter()
            .flatten()
            .flat_map(f32::to_le_bytes)
            .collect();
        memory.write(0x5080, &vertices).unwrap();
        memory.write(0x5500, &[0, 0, 0, 0, 0, 0, 0, 0x40]).unwrap(); // Depths 0 and 2.
        // Replace the actual MOV depth load with scalar ALD from attribute 1.
        let ald = 0xefd8000000000000u64 | (255 << 39) | (7 << 16) | (255 << 8) | 9 | (0x90 << 20);
        memory.write(0x5160, &ald.to_le_bytes()).unwrap();
        let mut captured = state.clone();
        for (register, value) in [
            (
                method::VERTEX_ATTRIBUTE_BASE + 1,
                1 | (0x12 << 21) | (7 << 27),
            ),
            (method::VERTEX_STREAM_BASE + 4, 0x1004),
            (method::VERTEX_STREAM_BASE + 5, 0),
            (method::VERTEX_STREAM_BASE + 6, 0x101500),
            (method::VERTEX_STREAM_BASE + 7, 2),
            (method::VERTEX_STREAM_LIMIT_BASE + 2, 0),
            (method::VERTEX_STREAM_LIMIT_BASE + 3, 0x101507),
            (method::VERTEX_STREAM_PER_INSTANCE_BASE + 1, 1),
        ] {
            captured.raw_registers.insert(register, value);
        }
        captured.primitive_raw = Some(4 | method::BEGIN_INSTANCE_CONTINUE);
        let background = [17, 43, 91, 255].repeat(32 * 32);
        for instance in [0, 1, 2, 3, 0] {
            captured.instance_id = Some(instance);
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &captured,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(captured.clone()),
            };
            renderer
                .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                .unwrap();
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &captured,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                pixels,
            );
            if instance < 2 {
                assert!(
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel == [51, 204, 255, 255])
                );
            } else {
                assert_eq!(pixels, background);
            }
        }
        memory.write(0x5160, &saved_instruction).unwrap();
        // Typed integer attributes must deliver register words, not normalized
        // floats or float casts. I2F is a separate encoded instruction.
        let mut integer_state = captured.clone();
        integer_state.instance_id = Some(0);
        integer_state
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE + 1, 0);
        integer_state
            .raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE + 3, 0x10150b);
        memory.write(0x5160, &ald.to_le_bytes()).unwrap();
        for (format_bits, width) in [(0x1d, 1u32), (0x1b, 2), (0x12, 4)] {
            integer_state
                .raw_registers
                .insert(method::VERTEX_STREAM_BASE + 4, 0x1000 | width);
            integer_state.raw_registers.insert(
                method::VERTEX_STREAM_LIMIT_BASE + 3,
                0x101500 + 3 * width - 1,
            );
            for signed in [false, true] {
                let format_type = if signed { 3 } else { 4 };
                integer_state.raw_registers.insert(
                    method::VERTEX_ATTRIBUTE_BASE + 1,
                    1 | (format_bits << 21) | (format_type << 27),
                );
                for rounding in 0..4u64 {
                    let conversion = 0x5cb8000000000000u64
                        | (2 << 8)
                        | (2 << 10)
                        | (u64::from(signed) << 13)
                        | (7 << 16)
                        | (9 << 20)
                        | 9
                        | (rounding << 39);
                    memory.write(0x5168, &conversion.to_le_bytes()).unwrap();
                    for word in [0u32, if signed { u32::MAX } else { 2 }, 0] {
                        memory
                            .write(0x5500, &word.to_le_bytes()[..width as usize].repeat(3))
                            .unwrap();
                        exosphere_runtime::gpu::color_memory::write_color_layer(
                            memory,
                            gpu,
                            &integer_state,
                            0,
                            TextureFormat::Rgba8Unorm,
                            &background,
                        )
                        .unwrap();
                        let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                            raw: 3,
                            state: Box::new(integer_state.clone()),
                        };
                        renderer
                            .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                            .unwrap();
                        let pixels = renderer
                            .device
                            .readback_rgba8(renderer.images[0].3)
                            .unwrap();
                        assert_eq!(
                            exosphere_runtime::gpu::color_memory::read_color_layer(
                                memory,
                                gpu,
                                &integer_state,
                                0,
                                TextureFormat::Rgba8Unorm,
                            )
                            .unwrap(),
                            pixels
                        );
                        if word == 0 {
                            assert!(
                                pixels
                                    .chunks_exact(4)
                                    .any(|pixel| pixel == [51, 204, 255, 255])
                            );
                        } else {
                            assert_eq!(pixels, background);
                        }
                    }
                }
            }
        }
        memory.write(0x5168, &saved_w_instruction).unwrap();
        // ALD returns integer register bits. Feeding them directly to Position.z
        // distinguishes captured InstanceID from native instance zero. Additional
        // cases below insert I2F to verify numeric conversion, not a bitcast.
        let instance_ald = (ald & !(0x90 << 20)) | (0x2f8 << 20);
        memory.write(0x5160, &instance_ald.to_le_bytes()).unwrap();
        let cases = [0, 0x40000000, 0]
            .into_iter()
            .map(|instance| (None, instance))
            .chain([false, true].into_iter().flat_map(|signed| {
                (0..4u64).flat_map(move |rounding| {
                    let clipped = if signed { u32::MAX } else { 2 };
                    [0, clipped, 0].map(move |instance| (Some((rounding, signed)), instance))
                })
            }));
        for (conversion_control, instance) in cases {
            if let Some((rounding, signed)) = conversion_control {
                // ALD slot 0 already supplies W=1 in this fixture. Its redundant
                // MOV can become a real I2F R9,R9 without changing scheduling.
                let conversion = 0x5cb8000000000000u64
                    | (2 << 8)
                    | (2 << 10)
                    | (7 << 16)
                    | (9 << 20)
                    | 9
                    | (u64::from(signed) << 13)
                    | (rounding << 39);
                memory.write(0x5168, &conversion.to_le_bytes()).unwrap();
            } else {
                memory.write(0x5168, &saved_w_instruction).unwrap();
            }
            captured.instance_id = Some(instance);
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &captured,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(captured.clone()),
            };
            renderer
                .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                .unwrap();
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &captured,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                pixels
            );
            if instance == 0 {
                assert!(
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel == [51, 204, 255, 255])
                );
            } else {
                assert_eq!(pixels, background);
            }
        }
        memory.write(0x5168, &saved_w_instruction).unwrap();
        memory.write(0x5160, &saved_instruction).unwrap();
        let vertex_ald = (ald & !(0x90 << 20)) | (0x2fc << 20);
        memory.write(0x5160, &vertex_ald.to_le_bytes()).unwrap();
        captured
            .raw_registers
            .insert(method::SET_VERTEX_ID_REPLACE, 0);
        captured
            .raw_registers
            .insert(method::SET_VERTEX_ID_GEN_MODE, 0);
        let cases = [0, 0x40000000, 0]
            .into_iter()
            .map(|base| (None, base))
            .chain([false, true].into_iter().flat_map(|signed| {
                (0..4u64).flat_map(move |rounding| {
                    // With indices 0/1/2, these bases put all vertices beyond
                    // the far plane (unsigned) or behind the near plane (signed).
                    let clipped = if signed { (-4i32) as u32 } else { 2 };
                    [0, clipped, 0].map(move |base| (Some((rounding, signed)), base))
                })
            }));
        for (conversion_control, base) in cases {
            if let Some((rounding, signed)) = conversion_control {
                let conversion = 0x5cb8000000000000u64
                    | (2 << 8)
                    | (2 << 10)
                    | (7 << 16)
                    | (9 << 20)
                    | 9
                    | (u64::from(signed) << 13)
                    | (rounding << 39);
                memory.write(0x5168, &conversion.to_le_bytes()).unwrap();
            } else {
                memory.write(0x5168, &saved_w_instruction).unwrap();
            }
            captured
                .raw_registers
                .insert(method::SET_VERTEX_ID_BASE, base);
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &captured,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(captured.clone()),
            };
            renderer
                .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                .unwrap();
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &captured,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                pixels
            );
            if base == 0 {
                assert!(
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel == [51, 204, 255, 255])
                );
            } else {
                assert_eq!(pixels, background);
            }
        }
        memory.write(0x5168, &saved_w_instruction).unwrap();
        memory.write(0x5160, &saved_instruction).unwrap();
        let mut saved_uniform = [0; 4];
        memory.read(0x5000, &mut saved_uniform).unwrap();
        #[derive(Clone, Copy)]
        enum ConversionSource {
            Uniform,
            Zero,
            Immediate,
        }
        let numeric_cases = || {
            [false, true].into_iter().flat_map(|signed| {
                (0..4u64).flat_map(move |rounding| {
                    [0, if signed { u32::MAX } else { 2 }, 0]
                        .map(move |word| (signed, rounding, word))
                })
            })
        };
        let cases = numeric_cases()
            .map(|(signed, rounding, word)| (signed, rounding, word, ConversionSource::Uniform))
            .chain([false, true].into_iter().flat_map(|signed| {
                (0..4u64).map(move |rounding| (signed, rounding, 0, ConversionSource::Zero))
            }))
            .chain(numeric_cases().map(|(signed, rounding, word)| {
                (signed, rounding, word, ConversionSource::Immediate)
            }));
        for (signed, rounding, word, source) in cases {
            let source_encoding = match source {
                ConversionSource::Uniform => 0x4cb8000000000000u64, // I2F R9,c[0][0].
                ConversionSource::Zero => 0x5cb8000000000000u64 | (255 << 20), // I2F R9,RZ.
                ConversionSource::Immediate => {
                    0x38b8000000000000u64
                        | (u64::from(word & 0x7ffff) << 20)
                        | (u64::from((word >> 19) & 1) << 56)
                }
            };
            let conversion = source_encoding
                | (2 << 8)
                | (2 << 10)
                | (u64::from(signed) << 13)
                | (7 << 16)
                | 9
                | (rounding << 39);
            memory.write(0x5160, &conversion.to_le_bytes()).unwrap();
            // Non-memory forms must not accidentally reuse the uniform source.
            let uniform_word = if matches!(source, ConversionSource::Uniform) {
                word
            } else {
                u32::MAX
            };
            memory.write(0x5000, &uniform_word.to_le_bytes()).unwrap();
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &captured,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(captured.clone()),
            };
            renderer
                .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                .unwrap();
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &captured,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                pixels
            );
            if word == 0 {
                assert!(
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel == [51, 204, 255, 255])
                );
            } else {
                assert_eq!(pixels, background);
            }
        }
        memory.write(0x5000, &saved_uniform).unwrap();
        memory.write(0x5160, &saved_instruction).unwrap();
        let mut saved_fragment_instruction = [0; 8];
        let mut saved_color_word = [0; 4];
        memory
            .read(0x5358, &mut saved_fragment_instruction)
            .unwrap();
        memory.read(0x5004, &mut saved_color_word).unwrap();
        // Vertex depth and fragment color are separate words in the same real
        // allocation. Stage-group ownership must preserve both bindings.
        memory.write(0x5000, &0u32.to_le_bytes()).unwrap();
        captured.constant_buffers.insert(
            (programs.groups.fragment_group, 0),
            captured.constant_buffers[&(programs.groups.vertex_group, 0)],
        );
        for source in [ConversionSource::Uniform, ConversionSource::Immediate] {
            for signed in [false, true] {
                for rounding in 0..4u64 {
                    for word in [0u32, 1, 0] {
                        let source_encoding = match source {
                            ConversionSource::Uniform => 0x4cb8000000000000u64 | (1 << 20), // c[0][4].
                            ConversionSource::Immediate => {
                                0x38b8000000000000u64 | (u64::from(word) << 20)
                            }
                            ConversionSource::Zero => unreachable!(),
                        };
                        let conversion = source_encoding
                            | (2 << 8)
                            | (2 << 10)
                            | (u64::from(signed) << 13)
                            | (7 << 16)
                            | (rounding << 39); // R0 export.
                        memory.write(0x5358, &conversion.to_le_bytes()).unwrap();
                        let uniform_word = if matches!(source, ConversionSource::Uniform) {
                            word
                        } else {
                            2
                        };
                        memory.write(0x5004, &uniform_word.to_le_bytes()).unwrap();
                        exosphere_runtime::gpu::color_memory::write_color_layer(
                            memory,
                            gpu,
                            &captured,
                            0,
                            TextureFormat::Rgba8Unorm,
                            &background,
                        )
                        .unwrap();
                        let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                            raw: 3,
                            state: Box::new(captured.clone()),
                        };
                        renderer
                            .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                            .unwrap();
                        let pixels = renderer
                            .device
                            .readback_rgba8(renderer.images[0].3)
                            .unwrap();
                        assert_eq!(
                            exosphere_runtime::gpu::color_memory::read_color_layer(
                                memory,
                                gpu,
                                &captured,
                                0,
                                TextureFormat::Rgba8Unorm,
                            )
                            .unwrap(),
                            pixels
                        );
                        let color = [if word == 0 { 0 } else { 255 }, 204, 255, 255];
                        assert!(pixels.chunks_exact(4).any(|pixel| pixel == color));
                        assert!(
                            pixels
                                .chunks_exact(4)
                                .all(|pixel| pixel == color || pixel == &background[..4])
                        );
                    }
                }
            }
        }
        memory.write(0x5358, &saved_fragment_instruction).unwrap();
        memory.write(0x5004, &saved_color_word).unwrap();
        memory.write(0x5000, &0u32.to_le_bytes()).unwrap();
        for attribute in [0x80u64, 0x88, 0x94] {
            // Export actual Position.x as a user varying. The ALD already
            // supplies W=1, so its redundant MOV can become scalar AST.
            let store = 0xeff0000000000000u64
                | (255 << 39)
                | (7 << 16)
                | (255 << 8)
                | 7
                | (attribute << 20);
            let interpolate = 0xe000000000000000u64
                | (255 << 8)
                | (255 << 20)
                | (255 << 39)
                | (7 << 47)
                | (7 << 16)
                | (attribute << 28);
            memory.write(0x5168, &store.to_le_bytes()).unwrap();
            memory.write(0x5358, &interpolate.to_le_bytes()).unwrap();
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &captured,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::new(captured.clone()),
            };
            renderer
                .draw(memory, gpu, &event, &programs.windows, &programs.groups)
                .unwrap();
            let pixels = renderer
                .device
                .readback_rgba8(renderer.images[0].3)
                .unwrap();
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &captured,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                pixels
            );
            let mut red_values = std::collections::BTreeSet::new();
            for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                if pixel == &background[..4] {
                    continue;
                }
                assert_eq!(&pixel[1..], &[204, 255, 255]);
                let x = (index % 32) as f32 + 0.5;
                let expected_red = ((x / 16.0 - 1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                assert!(
                    pixel[0].abs_diff(expected_red) <= 1,
                    "linear varying at pixel {index}: {pixel:?}, red {expected_red}"
                );
                red_values.insert(pixel[0]);
            }
            assert!(
                red_values.len() > 4,
                "varying must interpolate, not return a fixed vertex color"
            );
        }
        memory.write(0x5358, &saved_fragment_instruction).unwrap();
        memory.write(0x5168, &saved_w_instruction).unwrap();
        memory.write(0x5000, &saved_uniform).unwrap();
        memory.write(0x5080, &saved_vertices).unwrap();
    }

    /// Share the encoded fixture between standalone and authenticated adapters.
    /// The callback must return completed native pixels, not synthesized output.
    fn verify_vertex_uniform_fixture(
        memory: &mut AddressSpace,
        gpu: &NvGpuAddressSpace,
        cpu_base: u64,
        mut draw: impl FnMut(
            &mut AddressSpace,
            &NvGpuAddressSpace,
            &exosphere_gpu_ir::MaxwellEvent,
            &VerifiedDrawPrograms,
        ) -> Vec<u8>,
    ) {
        let resources = exosphere_runtime::gpu::resource_memory::GuestGpuResourceCatalog::new(100)
            .refresh(memory, gpu)
            .unwrap();
        let mut snapshot = DrawStateSnapshot::default();
        snapshot
            .raw_registers
            .insert(exosphere_gpu_ir::maxwell_state::method::SET_CT_SELECT, 1);
        snapshot.raw_registers.insert(
            exosphere_gpu_ir::maxwell_state::method::SET_CT_MRT_ENABLE,
            0,
        );
        for (field, word) in [0, 0x100000, 32, 32, 0xd5, 0, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            snapshot.raw_registers.insert(
                exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + field as u16,
                word,
            );
        }
        let vertex = resources.address_space.resolve(0x101080, 48).unwrap();
        let uniform = resources.address_space.resolve(0x200000, 16).unwrap();
        assert_eq!(uniform.resource, vertex.resource);
        let background: Vec<_> = [17, 43, 91, 255].into_iter().cycle().take(4096).collect();
        use exosphere_gpu_ir::maxwell_state::method;
        snapshot.primitive_raw = Some(4);
        for (register, word) in [
            (method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 0),
            (method::OGL_SET_CULL, 0),
            (method::OGL_SET_FRONT_FACE, 0x901),
            (method::SET_FRONT_POLYGON_MODE, 0x1b02),
            (method::SET_BACK_POLYGON_MODE, 0x1b02),
            (method::SET_RASTER_ENABLE, 1),
            (method::SET_POLY_OFFSET_FILL, 0),
            (method::SET_VIEWPORT_CLIP_CONTROL, 0x1801),
            (method::SET_WINDOW_OFFSET_X, 0),
            (method::SET_WINDOW_OFFSET_Y, 0),
            (method::SET_WINDOW_ORIGIN, 0),
            (method::SET_VIEWPORT_SCALE_OFFSET, 1),
            (method::SET_DEPTH_TEST, 0),
            (method::SET_DEPTH_WRITE, 0),
            (method::SET_STENCIL_TEST, 0),
            (method::SET_SINGLE_ROP_CONTROL, 0),
            (method::SET_LOGIC_OP, 0),
            (method::SET_BLEND_BASE, 0),
            (method::SET_CT_WRITE_BASE, 0x1111),
            (method::SET_PS_OUTPUT_SAMPLE_MASK_USAGE, 0),
            (method::SET_SAMPLE_MASK, 0),
            (method::SET_ANTI_ALIAS_ENABLE, 0),
            (method::SET_ANTI_ALIAS_ALPHA_CONTROL, 0),
            (method::SET_ANTI_ALIAS, 0),
            (method::SET_ALPHA_TO_COVERAGE_OVERRIDE, 0),
            (method::SCISSOR_BASE, 0),
            (method::VERTEX_ATTRIBUTE_BASE, (1 << 21) | (7 << 27)),
            (method::VERTEX_STREAM_BASE, 0x1010),
            (method::VERTEX_STREAM_PER_INSTANCE_BASE, 0),
            (method::VERTEX_STREAM_BASE + 1, 0),
            (method::VERTEX_STREAM_BASE + 2, 0x101080),
            (method::VERTEX_STREAM_BASE + 3, 0),
            (method::VERTEX_STREAM_LIMIT_BASE, 0),
            (method::VERTEX_STREAM_LIMIT_BASE + 1, 0x1010af),
        ] {
            snapshot.raw_registers.insert(register, word);
        }
        for (field, word) in [
            16.0f32.to_bits(),
            16.0f32.to_bits(),
            1.0f32.to_bits(),
            16.0f32.to_bits(),
            16.0f32.to_bits(),
            0,
            0x6420,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            snapshot
                .raw_registers
                .insert(method::VIEWPORT_BASE + field as u16, word);
        }
        let attributes = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
            slot: 0,
            location: 0,
            format: exosphere_gpu_ir::resource::VertexFormat::Rgba32Float,
        }];
        snapshot.constant_buffers.insert(
            (0, 0),
            exosphere_gpu_ir::maxwell_state::ConstantBufferBinding {
                size_raw: 16,
                address: exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200000 },
                bind_raw: 1,
            },
        );
        let operands = (3u64 << 47) | (255 << 39) | (7 << 16) | (255 << 8) | 7;
        let instructions = [
            0u64,                                            // Maxwell scheduling bundle.
            0xefd8000000000000 | operands | (0x80 << 20),    // ALD attribute slot 0.
            0x4c98000000000000 | (15 << 39) | (7 << 16) | 9, // MOV R9, c[0][0].
            0x0100000000000000 | (0x3f800000u64 << 20) | (7 << 16) | (15 << 12) | 10,
            0,
            0xeff0000000000000 | (operands & !(3 << 47)) | (1 << 47) | (0x70 << 20),
            0xeff0000000000000 | (operands & !((3 << 47) | 0xff)) | (1 << 47) | (0x78 << 20) | 9,
            0xe30000000007000f,
        ];
        let mut shader_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        shader_bytes.extend(instructions.into_iter().flat_map(u64::to_le_bytes));
        memory.write(cpu_base + 0x1100, &shader_bytes).unwrap();
        let mut fragment_bytes = vec![0; exosphere_gpu_ir::maxwell_shader::GRAPHICS_HEADER_SIZE];
        // Header exports RGBA from R0..R3, not the last constant defined.
        fragment_bytes[72..76].copy_from_slice(&15u32.to_le_bytes());
        let mov =
            |dst: u64, value: f32| 0x010000000007f000 | (u64::from(value.to_bits()) << 20) | dst;
        fragment_bytes.extend(
            [
                0,
                mov(0, 0.2),
                mov(1, 0.8),
                mov(2, 1.0),
                0,
                0x5c98000000000000 | (15 << 39) | (2 << 20) | (7 << 16) | 3,
                0x3898000000000000 | (1 << 56) | (15 << 39) | (0x7ffff << 20) | (7 << 16) | 7,
                0xe30000000007000f,
            ]
            .into_iter()
            .flat_map(u64::to_le_bytes),
        );
        memory.write(cpu_base + 0x1300, &fragment_bytes).unwrap();
        snapshot
            .raw_registers
            .insert(method::SET_PROGRAM_REGION_A, 0);
        snapshot
            .raw_registers
            .insert(method::SET_PROGRAM_REGION_B, 0x200000);
        for (slot, select, offset) in [(0, 0x11, 0x100), (4, 0x51, 0x300)] {
            let base = method::PIPELINE_BASE + slot * method::PIPELINE_STRIDE;
            snapshot.raw_registers.insert(base, select);
            snapshot.raw_registers.insert(base + 1, offset);
        }
        let direct = exosphere_runtime::gpu::shader_memory::prepare_discovered_direct_draw_shaders(
            memory,
            gpu,
            &snapshot,
            &[
                exosphere_gpu_ir::compiler::ShaderWindow {
                    gpu_address: 0x200100,
                    bytes: shader_bytes.len(),
                },
                exosphere_gpu_ir::compiler::ShaderWindow {
                    gpu_address: 0x200300,
                    bytes: fragment_bytes.len(),
                },
            ],
            &exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups {
                vertex_group: 0,
                fragment_group: 4,
                vertex_set: 0,
                fragment_set: 1,
            },
            &resources.address_space,
        )
        .unwrap();
        assert_eq!(direct.attributes.len(), attributes.len());
        assert_eq!(direct.attributes[0].slot, attributes[0].slot);
        assert_eq!(direct.attributes[0].location, attributes[0].location);
        assert_eq!(direct.attributes[0].format, attributes[0].format);
        let prepared = direct.shaders;
        assert_eq!(prepared.vertex.shader.address, 0x200100);
        assert_eq!(prepared.fragment.shader.address, 0x200300);
        assert_eq!(prepared.vertex.shader.graphics_source.unwrap().slot, 0);
        assert_eq!(prepared.fragment.shader.graphics_source.unwrap().slot, 4);
        assert_eq!(prepared.vertex.shader.stats.opaque, 0);
        assert_eq!(prepared.fragment.shader.stats.opaque, 0);
        let allocation_event = exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
            first: 0,
            count: 3,
            state: Box::new(snapshot.clone()),
        };
        let draw_allocations = resources
            .prepare_draw_allocations(&allocation_event, &prepared, &direct.attributes)
            .unwrap();
        assert_eq!(draw_allocations.len(), 1);
        assert_eq!(
            draw_allocations[0].usage,
            exosphere_gpu_ir::resource::BufferUsage::VertexUniform
        );
        assert_eq!(draw_allocations[0].size, 2 * PAGE_SIZE);
        // Three input roles alias one canonical allocation, not three copies.
        memory
            .write(cpu_base + 0x1400, &[0, 0, 1, 0, 2, 0])
            .unwrap();
        let mut indexed_state = snapshot.clone();
        indexed_state.index_buffer.address =
            exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200400 };
        indexed_state.index_buffer.limit =
            exosphere_gpu_ir::maxwell_state::RawGpuAddress { a: 0, b: 0x200405 };
        indexed_state.index_buffer.index_size_raw = 1;
        indexed_state.index_buffer.first = 0;
        indexed_state
            .raw_registers
            .insert(method::SET_VERTEX_ELEMENT_BASE, 0);
        indexed_state
            .raw_registers
            .insert(method::SET_PRIMITIVE_RESTART, 0);
        let indexed_event = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
            raw: 3,
            state: Box::new(indexed_state),
        };
        let mut indexed =
            exosphere_runtime::gpu::index_memory::prepare_captured_indexed_vertex_inputs(
                memory,
                gpu,
                &indexed_event,
                &direct.attributes,
                &resources.address_space,
            )
            .unwrap();
        let indexed_allocations = resources
            .prepare_indexed_draw_allocations(&indexed, &prepared)
            .unwrap();
        assert_eq!(indexed_allocations.len(), 1);
        assert_eq!(indexed_allocations[0].id, draw_allocations[0].id);
        assert_eq!(indexed_allocations[0].size, 2 * PAGE_SIZE);
        assert_eq!(
            indexed_allocations[0].usage,
            exosphere_gpu_ir::resource::BufferUsage::IndexVertexUniform
        );
        assert_eq!(indexed.index_range.unwrap().offset, 0x1400);
        assert_eq!(indexed.index_range.unwrap().length, 6);
        let exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } = &indexed_event else {
            unreachable!()
        };
        let (vertex_input, _) =
            exosphere_gpu_ir::maxwell_state::merge_vertex_inputs(&indexed.inputs).unwrap();
        let mut commands = indexed.draw_commands().unwrap();
        let indexed_draw = exosphere_gpu_ir::pipeline::PreparedVertexArrayDraw {
            topology: exosphere_gpu_ir::pipeline::draw_topology(state).unwrap(),
            vertex_input,
            draw: commands.pop().unwrap(),
            bindings: commands,
            vertex_ranges: indexed.inputs.iter().map(|input| input.range).collect(),
        };
        let module = exosphere_gpu_ir::compiler::prepare_color_indexed_module(
            state,
            &exosphere_gpu_ir::compiler::ColorVertexArrayInputs {
                pipeline_id: 1,
                image: ResourceId(u64::MAX),
                vertex: &prepared.vertex,
                fragment: &prepared.fragment,
                attributes: &direct.attributes,
                allocations: &indexed_allocations,
            },
            &resources.address_space,
            indexed_draw,
            indexed.index_range.unwrap(),
        )
        .unwrap();
        assert_eq!(module.buffers, indexed_allocations);
        assert!(module.commands.iter().any(|command| matches!(
            command,
            exosphere_gpu_ir::Command::DrawIndexed {
                indices: 3,
                first_index: 0,
                ..
            }
        )));
        // A prepared binding cannot be reused after its resource offset changes.
        if let Some(exosphere_gpu_ir::Command::BindIndexBuffer { offset, .. }) =
            &mut indexed.index_binding
        {
            *offset += 2;
        }
        assert!(
            resources
                .prepare_indexed_draw_allocations(&indexed, &prepared)
                .is_err()
        );
        for (positions, depth, visible) in [
            (
                [
                    [-0.5f32, -0.5, 0.0, 1.0],
                    [0.5, -0.5, 0.0, 1.0],
                    [0.0, 0.5, 0.0, 1.0],
                ],
                0.0f32,
                true,
            ),
            (
                [
                    [-0.5f32, -0.5, 0.0, 1.0],
                    [0.5, -0.5, 0.0, 1.0],
                    [0.0, 0.5, 0.0, 1.0],
                ],
                2.0f32,
                false,
            ),
            ([[2.0f32, 2.0, 0.0, 1.0]; 3], 0.0f32, false),
        ] {
            let bytes: Vec<_> = positions
                .into_iter()
                .flatten()
                .flat_map(f32::to_le_bytes)
                .collect();
            memory.write(cpu_base + 0x1080, &bytes).unwrap();
            memory
                .write(cpu_base + 0x1000, &depth.to_le_bytes())
                .unwrap();
            exosphere_runtime::gpu::color_memory::write_color_layer(
                memory,
                gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
                &background,
            )
            .unwrap();
            let event = exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
                first: 0,
                count: 3,
                state: Box::new(snapshot.clone()),
            };
            let provider = DirectDrawProgramProvider::new(
                4096,
                exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups {
                    vertex_group: 0,
                    fragment_group: 4,
                    vertex_set: 0,
                    fragment_set: 1,
                },
            )
            .unwrap();
            let programs = provider.resolve_programs(memory, gpu, &snapshot).unwrap();
            assert_eq!(programs.windows.len(), 2);
            let pixels = draw(memory, gpu, &event, &programs);
            assert_eq!(pixels != background, visible);
            for pixel in pixels.chunks_exact(4) {
                assert!(pixel == &background[..4] || (visible && pixel == [51, 204, 255, 255]));
            }
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &snapshot,
                    0,
                    TextureFormat::Rgba8Unorm
                )
                .unwrap(),
                pixels
            );
        }
    }

    #[test]
    #[ignore = "requires an explicitly selected Vulkan 1.3 device"]
    fn native_vulkan_pixels_reach_guest_tiled_memory() {
        let mut device = AshVulkanDevice::create().unwrap();
        let module = GpuModule::triangle_vulkan_smoke_test(32, 32);
        let image = module.textures[0].id;
        device
            .execute_plan_with_resources(
                &VulkanBackend::new().plan(&module).unwrap(),
                &mut InMemoryGpuResources::new(),
            )
            .unwrap();
        let expected = device.readback_rgba8(image).unwrap();
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let backing = NvMapBacking {
            address: 0x4000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        let mut gpu = NvGpuAddressSpace::default();
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &backing).unwrap();
        let mut snapshot = DrawStateSnapshot::default();
        snapshot
            .raw_registers
            .insert(exosphere_gpu_ir::maxwell_state::method::SET_CT_SELECT, 1);
        snapshot.raw_registers.insert(
            exosphere_gpu_ir::maxwell_state::method::SET_CT_MRT_ENABLE,
            0,
        );
        for (field, word) in [0, 0x100000, 32, 32, 0xd5, 0, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            snapshot.raw_registers.insert(
                exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + field as u16,
                word,
            );
        }
        let mut wrong_shape = snapshot.clone();
        wrong_shape.raw_registers.insert(
            exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + 2,
            64,
        );
        wrong_shape.raw_registers.insert(
            exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + 3,
            16,
        );
        assert!(matches!(
            readback_color_layer(&mut device, &mut memory, &gpu, &wrong_shape, 0, image,),
            Err(ColorReadbackError::ShapeMismatch)
        ));
        let mut untouched = vec![0xff; PAGE_SIZE as usize];
        gpu.read(&memory, 0x100000, &mut untouched).unwrap();
        assert!(untouched.iter().all(|byte| *byte == 0));
        readback_color_layer(&mut device, &mut memory, &gpu, &snapshot, 0, image).unwrap();
        let mut tiled = vec![0; PAGE_SIZE as usize];
        gpu.read(&memory, 0x100000, &mut tiled).unwrap();
        assert_eq!(
            exosphere_gpu_ir::block_linear::deswizzle_layers(&tiled, 128, 32, 1, 1, 0, 0).unwrap(),
            expected
        );
        assert!(
            expected
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
        let background: Vec<u8> = [17, 43, 91, 255].into_iter().cycle().take(4096).collect();
        exosphere_runtime::gpu::color_memory::write_color_layer(
            &mut memory,
            &gpu,
            &snapshot,
            0,
            TextureFormat::Rgba8Unorm,
            &background,
        )
        .unwrap();
        device = AshVulkanDevice::create().unwrap();
        create_color_layer(&mut device, &memory, &gpu, &snapshot, 0, image).unwrap();
        upload_color_layer(&mut device, &memory, &gpu, &snapshot, 0, image).unwrap();
        assert_eq!(device.readback_rgba8(image).unwrap(), background);
        assert!(device.upload_rgba8(image, &background[..4095]).is_err());
        assert_eq!(device.readback_rgba8(image).unwrap(), background);
        let mut preserving = module.clone();
        preserving
            .commands
            .retain(|command| !matches!(command, exosphere_gpu_ir::Command::ClearColor { .. }));
        memory
            .map(
                0x6000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let commands = NvMapBacking {
            address: 0x6000,
            alignment: 4096,
            flags: 1,
            kind: 0,
            heap_mask: 0,
            lease: memory
                .pin_device_memory(0x6000, PAGE_SIZE, MemoryPermission::READ_WRITE)
                .unwrap(),
        };
        gpu.map_fixed(0x200000, PAGE_SIZE, 0, &commands).unwrap();
        let mut words = vec![0x60010000u32, 0xb197];
        for (&method, &word) in &snapshot.raw_registers {
            words.extend([0x60010000 | u32::from(method), word]);
        }
        words.extend([
            0x60010000 | u32::from(exosphere_gpu_ir::maxwell_state::method::SET_VERTEX_ARRAY_START),
            0,
            0x60010000 | u32::from(exosphere_gpu_ir::maxwell_state::method::DRAW_VERTEX_ARRAY),
            3,
        ]);
        // FIFO release belongs after the draw, in separate command memory;
        // it must remain zero throughout rendering and color writeback.
        words.extend([0x20040004, 0, 0x200800, 42, 0x01000002]);
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        memory.write(0x6000, &bytes).unwrap();
        let mut channel = exosphere_runtime::gpu::queue::GuestGpuChannel::new(1).unwrap();
        channel
            .enqueue(&[0x200000 | ((words.len() as u64) << 42)])
            .unwrap();
        // Decode a live mapped pushbuffer and retire only after Vulkan/guest
        // writeback. Shader translation remains a host fixture, not game code.
        let resource_snapshot =
            exosphere_runtime::gpu::resource_memory::GuestGpuResourceCatalog::new(1000)
                .refresh(&memory, &gpu)
                .unwrap();
        let result = channel.step_with_draw_handler(&mut memory, &gpu, |memory, gpu, event, _| {
            let mut semaphore = [0; 4];
            gpu.read(memory, 0x200800, &mut semaphore).unwrap();
            assert_eq!(semaphore, [0; 4]);
            let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray { state, .. } = event else {
                unreachable!("fixture contains one vertex-array draw")
            };
            execute_color_draw_from_guest(
                &mut device,
                memory,
                gpu,
                ColorDrawTarget {
                    snapshot: state,
                    index: 0,
                    image,
                },
                &preserving,
                &resource_snapshot.address_space,
            )
            .map_err(|error| {
                exosphere_runtime::gpu::GuestGpuExecutionFailure::Draw(Box::new(error))
            })?;
            gpu.read(memory, 0x200800, &mut semaphore).unwrap();
            assert_eq!(semaphore, [0; 4]);
            // Successful rendering includes canonical writeback before the
            // handler returns and the executor reaches the release event.
            let guest_pixels = exosphere_runtime::gpu::color_memory::read_color_layer(
                memory,
                gpu,
                state,
                0,
                TextureFormat::Rgba8Unorm,
            )
            .unwrap();
            assert_eq!(guest_pixels, device.readback_rgba8(image).unwrap());
            Ok(())
        });
        assert_eq!(
            result,
            exosphere_runtime::gpu::queue::ChannelStep::Applied { transfers: 0 },
            "channel fault: {:?}",
            channel.fault()
        );
        assert_eq!(channel.retired_entries(), 1);
        assert_eq!(channel.pending_entries(), 0);
        assert!(channel.fault().is_none());
        let mut semaphore = [0; 4];
        gpu.read(&memory, 0x200800, &mut semaphore).unwrap();
        assert_eq!(u32::from_le_bytes(semaphore), 42);
        gpu.write(&mut memory, 0x200800, &[0; 4]).unwrap();
        let mut failed = exosphere_runtime::gpu::queue::GuestGpuChannel::new(1).unwrap();
        failed
            .enqueue(&[0x200000 | ((words.len() as u64) << 42)])
            .unwrap();
        assert_eq!(
            failed.step_with_draw_handler(&mut memory, &gpu, |_, _, _, _| {
                Err(exosphere_runtime::gpu::GuestGpuExecutionFailure::Draw(
                    Box::new(ColorReadbackError::UnsupportedDraw),
                ))
            }),
            exosphere_runtime::gpu::queue::ChannelStep::Stopped,
        );
        assert_eq!(failed.retired_entries(), 0);
        assert_eq!(failed.pending_entries(), 1);
        gpu.read(&memory, 0x200800, &mut semaphore).unwrap();
        assert_eq!(semaphore, [0; 4]);
        let rendered = device.readback_rgba8(image).unwrap();
        let mut preserved_pixels = 0;
        for (actual, reference) in rendered.chunks_exact(4).zip(expected.chunks_exact(4)) {
            if reference[..3] == [0, 0, 0] {
                assert_eq!(actual, &background[..4]);
                preserved_pixels += 1;
            } else {
                assert_eq!(actual, reference);
            }
        }
        assert!(preserved_pixels > 0);
        assert!(
            rendered
                .chunks_exact(4)
                .any(|pixel| pixel != &background[..4])
        );
        readback_color_layer(&mut device, &mut memory, &gpu, &snapshot, 0, image).unwrap();
        assert_eq!(
            exosphere_runtime::gpu::color_memory::read_color_layer(
                &memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
            )
            .unwrap(),
            rendered
        );
        let mut incompatible = preserving.clone();
        incompatible.commands.push(exosphere_gpu_ir::Command::Fill {
            destination: image,
            value: 0,
            bytes: 4,
        });
        assert!(matches!(
            execute_color_draw(
                &mut device,
                &mut memory,
                &gpu,
                ColorDrawTarget {
                    snapshot: &snapshot,
                    index: 0,
                    image
                },
                &incompatible,
                &mut InMemoryGpuResources::new(),
            ),
            Err(ColorReadbackError::UnsupportedDraw)
        ));
        assert_eq!(device.readback_rgba8(image).unwrap(), rendered);
        for case in 0..4 {
            let mut unsupported = preserving.clone();
            let fixed = &mut unsupported.graphics_pipelines[0].fixed_function;
            match case {
                0 => fixed.depth_stencil.depth_test = true,
                1 => fixed.depth_stencil.depth_write = true,
                2 => fixed.depth_stencil.stencil_test = true,
                _ => fixed.multisample.raster_samples = 2,
            }
            assert!(matches!(
                execute_color_draw(
                    &mut device,
                    &mut memory,
                    &gpu,
                    ColorDrawTarget {
                        snapshot: &snapshot,
                        index: 0,
                        image
                    },
                    &unsupported,
                    &mut InMemoryGpuResources::new(),
                ),
                Err(ColorReadbackError::UnsupportedDraw)
            ));
            assert_eq!(device.readback_rgba8(image).unwrap(), rendered);
            assert_eq!(
                exosphere_runtime::gpu::color_memory::read_color_layer(
                    &memory,
                    &gpu,
                    &snapshot,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap(),
                rendered
            );
        }
        gpu.unmap(0x100000).unwrap();
        for selection in [0, 2, 1 | (1 << 4), 0x10000001] {
            let mut mismatched = snapshot.clone();
            mismatched.raw_registers.insert(
                exosphere_gpu_ir::maxwell_state::method::SET_CT_SELECT,
                selection,
            );
            assert!(matches!(
                execute_color_draw(
                    &mut device,
                    &mut memory,
                    &gpu,
                    ColorDrawTarget {
                        snapshot: &mismatched,
                        index: 0,
                        image
                    },
                    &preserving,
                    &mut InMemoryGpuResources::new(),
                ),
                Err(ColorReadbackError::UnsupportedDraw)
            ));
            assert_eq!(device.readback_rgba8(image).unwrap(), rendered);
        }
        let readonly = NvMapBacking {
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
            ..backing
        };
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &readonly).unwrap();
        let before = exosphere_runtime::gpu::color_memory::read_color_layer(
            &memory,
            &gpu,
            &snapshot,
            0,
            TextureFormat::Rgba8Unorm,
        )
        .unwrap();
        assert!(matches!(
            execute_color_draw(
                &mut device,
                &mut memory,
                &gpu,
                ColorDrawTarget {
                    snapshot: &snapshot,
                    index: 0,
                    image
                },
                &preserving,
                &mut InMemoryGpuResources::new(),
            ),
            Err(ColorReadbackError::Guest(
                exosphere_runtime::gpu::color_memory::ColorWriteError::Memory(_)
            ))
        ));
        assert_eq!(device.readback_rgba8(image).unwrap(), rendered);
        assert_eq!(
            exosphere_runtime::gpu::color_memory::read_color_layer(
                &memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
            )
            .unwrap(),
            before
        );
    }
}
