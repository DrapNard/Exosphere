// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Conversion helpers for fixed-function Maxwell state.
//!
//! Raw guest GPU addresses are deliberately *not* converted to [`crate::ResourceId`] here. The returned
//! unresolved records must first pass through `GpuAddressSpace` and `ResourceTable` so aliases and
//! remaps preserve resource identity correctly.

use crate::{
    maxwell_state::{Maxwell3dState, RawGpuAddress},
    resource::{PrimitiveTopology, TextureFormat, TextureLayout},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnresolvedTextureDesc {
    pub address: RawGpuAddress,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub layers: u32,
    pub mip_levels: u32,
    pub format: TextureFormat,
    pub layout: TextureLayout,
}

pub fn topology(raw: u32) -> PrimitiveTopology {
    match raw {
        1 => PrimitiveTopology::PointList,
        2 => PrimitiveTopology::LineList,
        3 => PrimitiveTopology::LineStrip,
        4 => PrimitiveTopology::TriangleList,
        5 => PrimitiveTopology::TriangleStrip,
        other => PrimitiveTopology::Unknown(other),
    }
}

/// BEGIN uses a different numbering from SET_PRIMITIVE_TOPOLOGY for lines.
/// Flags for instance continuation/split primitives require additional handling
/// and are not silently stripped from the raw word.
pub fn begin_topology(raw: u32) -> PrimitiveTopology {
    match raw {
        0 => PrimitiveTopology::PointList,
        1 => PrimitiveTopology::LineList,
        3 => PrimitiveTopology::LineStrip,
        4 => PrimitiveTopology::TriangleList,
        5 => PrimitiveTopology::TriangleStrip,
        6 => PrimitiveTopology::TriangleFan,
        other => PrimitiveTopology::Unknown(other),
    }
}

/// Resolve only explicitly captured topology control, without inventing reset
/// values. Unknown topology words remain unknown, not a substituted triangle.
pub fn draw_topology(state: &crate::maxwell_state::DrawStateSnapshot) -> Option<PrimitiveTopology> {
    use crate::maxwell_state::method;
    match *state
        .raw_registers
        .get(&method::SET_PRIMITIVE_TOPOLOGY_CONTROL)?
    {
        // BEGIN's high bits control instance sequencing, not primitive kind.
        0 => Some(begin_topology(state.primitive_raw? & 0xffff)),
        1 => Some(topology(
            *state.raw_registers.get(&method::SET_PRIMITIVE_TOPOLOGY)?,
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedScissor {
    Disabled,
    Enabled(crate::resource::ScissorDesc),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedColorTargets {
    /// Physical target indices, in shader-output order. Never reorder this list.
    pub targets: Vec<u8>,
    pub mrt_enabled: bool,
}

pub fn captured_color_targets(
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Option<CapturedColorTargets> {
    use crate::maxwell_state::method;
    let raw = *state.raw_registers.get(&method::SET_CT_SELECT)?;
    let count = raw & 0xf;
    if count > 8 || raw & 0xf0000000 != 0 {
        return None;
    }
    let mrt_enabled = match *state.raw_registers.get(&method::SET_CT_MRT_ENABLE)? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let targets = (0..count)
        .map(|slot| ((raw >> (4 + slot * 3)) & 7) as u8)
        .collect();
    Some(CapturedColorTargets {
        targets,
        mrt_enabled,
    })
}

/// Assemble fixed state for explicitly selected active color slots. Inactive
/// slots remain placeholders; active state must always come from captured words.
/// Attachment discovery and memory resolution belong to draw assembly.
pub fn captured_fixed_function(
    state: &crate::maxwell_state::DrawStateSnapshot,
    active_targets: &[usize],
) -> Option<crate::resource::FixedFunctionState> {
    use crate::resource::{BlendState, FixedFunctionState};
    if active_targets.is_empty() || active_targets.len() > 8 {
        return None;
    }
    let raster = captured_raster(state)?;
    let depth_stencil = captured_depth_stencil(state)?;
    let multisample = captured_single_sample(state)?;
    let mut blend = BlendState::default();
    let mut seen = 0u8;
    for &index in active_targets {
        if index >= 8 || seen & (1 << index) != 0 {
            return None;
        }
        seen |= 1 << index;
        blend.targets[index] = captured_blend_target(state, index)?;
    }
    // Target preparation excludes enabled logic and constant blend factors.
    Some(FixedFunctionState {
        raster,
        depth_stencil,
        blend,
        multisample,
    })
}

/// Single-sample path without shader masks or alpha coverage overrides.
/// Other sampling conventions must be lowered explicitly, not reset to 1x.
pub fn captured_single_sample(
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Option<crate::resource::MultisampleState> {
    use crate::maxwell_state::method;
    for register in [
        method::SET_PS_OUTPUT_SAMPLE_MASK_USAGE,
        method::SET_ANTI_ALIAS_ENABLE,
        method::SET_ANTI_ALIAS_ALPHA_CONTROL,
        method::SET_ANTI_ALIAS,
        method::SET_ALPHA_TO_COVERAGE_OVERRIDE,
    ] {
        if *state.raw_registers.get(&register)? != 0 {
            return None;
        }
    }
    let sample_mask = match *state.raw_registers.get(&method::SET_SAMPLE_MASK)? {
        0 => u32::MAX,
        0x10 => {
            let mask = *state
                .raw_registers
                .get(&method::SET_COLOR_TARGET_SAMPLE_MASK)?;
            if mask & !0xffff != 0 {
                return None;
            }
            mask
        }
        _ => return None,
    };
    Some(crate::resource::MultisampleState {
        raster_samples: 1,
        sample_mask,
        alpha_to_coverage: false,
        alpha_to_one: false,
    })
}

/// Resolve one unblended target with independently captured component masks.
/// Enabled blending/logic operations and shared ROP state require other lowering.
pub fn captured_unblended_target(
    state: &crate::maxwell_state::DrawStateSnapshot,
    index: usize,
) -> Option<crate::resource::BlendTargetState> {
    let target = captured_blend_target(state, index)?;
    (!target.enable).then_some(target)
}

/// Shared blend equations with explicit per-target enable and component masks.
/// Constant/dual-source factors and per-target equations require more state.
pub fn captured_blend_target(
    state: &crate::maxwell_state::DrawStateSnapshot,
    index: usize,
) -> Option<crate::resource::BlendTargetState> {
    use crate::maxwell_state::method;
    if index >= 8
        || *state.raw_registers.get(&method::SET_SINGLE_ROP_CONTROL)? != 0
        || *state.raw_registers.get(&method::SET_LOGIC_OP)? != 0
    {
        return None;
    }
    let mask = *state
        .raw_registers
        .get(&(method::SET_CT_WRITE_BASE + index as u16))?;
    if mask & !0x1111 != 0 {
        return None;
    }
    let color_write_mask =
        ((mask & 1) | ((mask >> 3) & 2) | ((mask >> 6) & 4) | ((mask >> 9) & 8)) as u8;
    // Inactive factors need no capture. The enabled path replaces every factor
    // and equation below before returning any executable target state.
    let mut target = crate::resource::BlendTargetState {
        color_write_mask,
        ..Default::default()
    };
    match *state
        .raw_registers
        .get(&(method::SET_BLEND_BASE + index as u16))?
    {
        0 => return Some(target),
        1 => target.enable = true,
        _ => return None,
    }
    if *state
        .raw_registers
        .get(&method::SET_BLEND_STATE_PER_TARGET)?
        != 0
    {
        return None;
    }
    target.color_op = blend_op(*state.raw_registers.get(&method::SET_BLEND_COLOR_OP)?)?;
    target.alpha_op = blend_op(*state.raw_registers.get(&method::SET_BLEND_ALPHA_OP)?)?;
    target.src_color = blend_factor(
        *state
            .raw_registers
            .get(&method::SET_BLEND_COLOR_SOURCE_COEFF)?,
    )?;
    target.dst_color = blend_factor(
        *state
            .raw_registers
            .get(&method::SET_BLEND_COLOR_DEST_COEFF)?,
    )?;
    target.src_alpha = blend_factor(
        *state
            .raw_registers
            .get(&method::SET_BLEND_ALPHA_SOURCE_COEFF)?,
    )?;
    target.dst_alpha = blend_factor(
        *state
            .raw_registers
            .get(&method::SET_BLEND_ALPHA_DEST_COEFF)?,
    )?;
    Some(target)
}

fn blend_op(raw: u32) -> Option<crate::resource::BlendOp> {
    use crate::resource::BlendOp;
    Some(match raw {
        1 | 0x8006 => BlendOp::Add,
        2 | 0x800a => BlendOp::Subtract,
        3 | 0x800b => BlendOp::ReverseSubtract,
        4 | 0x8007 => BlendOp::Min,
        5 | 0x8008 => BlendOp::Max,
        _ => return None,
    })
}

fn blend_factor(raw: u32) -> Option<crate::resource::BlendFactor> {
    use crate::resource::BlendFactor;
    Some(match raw {
        1 | 0x4000 => BlendFactor::Zero,
        2 | 0x4001 => BlendFactor::One,
        3 | 0x4300 => BlendFactor::SrcColor,
        4 | 0x4301 => BlendFactor::OneMinusSrcColor,
        5 | 0x4302 => BlendFactor::SrcAlpha,
        6 | 0x4303 => BlendFactor::OneMinusSrcAlpha,
        7 | 0x4304 => BlendFactor::DstAlpha,
        8 | 0x4305 => BlendFactor::OneMinusDstAlpha,
        9 | 0x4306 => BlendFactor::DstColor,
        10 | 0x4307 => BlendFactor::OneMinusDstColor,
        11 | 0x4308 => BlendFactor::SrcAlphaSaturate,
        _ => return None,
    })
}

/// Preserve depth controls without synthesizing an enabled stencil interface.
/// Stencil faces below are inactive placeholders only when stencil is disabled.
pub fn captured_depth_stencil(
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Option<crate::resource::DepthStencilState> {
    use crate::{
        maxwell_state::method,
        resource::{CompareOp, DepthStencilState},
    };
    let read_bool = |register| match state.raw_registers.get(&register)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    let depth_test = read_bool(method::SET_DEPTH_TEST)?;
    let depth_write = read_bool(method::SET_DEPTH_WRITE)?;
    if read_bool(method::SET_STENCIL_TEST)? {
        return None;
    }
    let depth_compare = if depth_test {
        match *state.raw_registers.get(&method::SET_DEPTH_FUNC)? {
            1 | 0x200 => CompareOp::Never,
            2 | 0x201 => CompareOp::Less,
            3 | 0x202 => CompareOp::Equal,
            4 | 0x203 => CompareOp::LessOrEqual,
            5 | 0x204 => CompareOp::Greater,
            6 | 0x205 => CompareOp::NotEqual,
            7 | 0x206 => CompareOp::GreaterOrEqual,
            8 | 0x207 => CompareOp::Always,
            _ => return None,
        }
    } else {
        CompareOp::Always
    };
    Some(DepthStencilState {
        depth_test,
        depth_write,
        depth_compare,
        stencil_test: false,
        front: Default::default(),
        back: Default::default(),
    })
}

/// Only face selection and polygon mode; other raster controls are not defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedFaces {
    pub cull: crate::resource::CullMode,
    pub front_face: crate::resource::FrontFace,
    pub polygon_mode: crate::resource::PolygonMode,
}

pub fn captured_faces(state: &crate::maxwell_state::DrawStateSnapshot) -> Option<CapturedFaces> {
    use crate::{
        maxwell_state::method,
        resource::{CullMode, FrontFace, PolygonMode},
    };
    let read = |register| state.raw_registers.get(&register).copied();
    let cull = match read(method::OGL_SET_CULL)? {
        0 => CullMode::None,
        1 => match read(method::OGL_SET_CULL_FACE)? {
            0x404 => CullMode::Front,
            0x405 => CullMode::Back,
            0x408 => CullMode::FrontAndBack,
            _ => return None,
        },
        _ => return None,
    };
    let front_face = match read(method::OGL_SET_FRONT_FACE)? {
        0x900 => FrontFace::Clockwise,
        0x901 => FrontFace::CounterClockwise,
        _ => return None,
    };
    let front = read(method::SET_FRONT_POLYGON_MODE)?;
    // Vulkan's basic raster state cannot represent independent front/back modes.
    if read(method::SET_BACK_POLYGON_MODE)? != front {
        return None;
    }
    let polygon_mode = match front {
        0x1b00 => PolygonMode::Point,
        0x1b01 => PolygonMode::Line,
        0x1b02 => PolygonMode::Fill,
        _ => return None,
    };
    Some(CapturedFaces {
        cull,
        front_face,
        polygon_mode,
    })
}

/// Assemble the supported raster controls from explicit captured words.
/// Enabled bias requires explicit format-dependent control and all coefficients.
pub fn captured_raster(
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Option<crate::resource::RasterState> {
    use crate::{maxwell_state::method, resource::PolygonMode};
    let faces = captured_faces(state)?;
    let rasterizer_discard = match *state.raw_registers.get(&method::SET_RASTER_ENABLE)? {
        0 => true,
        1 => false,
        _ => return None,
    };
    if *state
        .raw_registers
        .get(&method::SET_VIEWPORT_CLIP_CONTROL)?
        != 0x1801
    {
        return None;
    }
    let bias_method = match faces.polygon_mode {
        PolygonMode::Point => method::SET_POLY_OFFSET_POINT,
        PolygonMode::Line => method::SET_POLY_OFFSET_LINE,
        PolygonMode::Fill => method::SET_POLY_OFFSET_FILL,
        PolygonMode::Unknown(_) => return None,
    };
    let (depth_bias, depth_bias_factors) = match *state.raw_registers.get(&bias_method)? {
        0 => (false, crate::resource::DepthBiasFactors::default()),
        1 => (true, captured_depth_bias(state)?),
        _ => return None,
    };
    Some(crate::resource::RasterState {
        cull: faces.cull,
        front_face: faces.front_face,
        polygon_mode: faces.polygon_mode,
        depth_clamp: false,
        rasterizer_discard,
        depth_bias,
        depth_bias_factors,
    })
}

/// Resolve the format-dependent polygon-offset path only. Nouveau programs the
/// hardware constant coefficient as twice its API units (nvc0_state.c); reverse
/// that scaling here. Absolute bias needs a different depth representation.
pub fn captured_depth_bias(
    state: &crate::maxwell_state::DrawStateSnapshot,
) -> Option<crate::resource::DepthBiasFactors> {
    use crate::maxwell_state::method;
    if *state.raw_registers.get(&method::SET_DEPTH_BIAS_CONTROL)? != 1 {
        return None;
    }
    let constant = f32::from_bits(*state.raw_registers.get(&method::SET_DEPTH_BIAS)?);
    let slope_bits = *state
        .raw_registers
        .get(&method::SET_SLOPE_SCALE_DEPTH_BIAS)?;
    let clamp_bits = *state.raw_registers.get(&method::SET_DEPTH_BIAS_CLAMP)?;
    if ![
        constant,
        f32::from_bits(slope_bits),
        f32::from_bits(clamp_bits),
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return None;
    }
    Some(crate::resource::DepthBiasFactors {
        constant_bits: (constant * 0.5).to_bits(),
        slope_bits,
        clamp_bits,
    })
}

/// Convert the supported zero-to-one, identity-swizzle viewport transform.
/// Other clip conventions require shader/pipeline support, not guessed defaults.
/// This does not assemble the remaining rasterization/depth state.
pub fn captured_viewport(
    state: &crate::maxwell_state::DrawStateSnapshot,
    index: usize,
) -> Option<crate::resource::ViewportDesc> {
    use crate::maxwell_state::method;
    if index >= method::VIEWPORT_COUNT {
        return None;
    }
    for (register, expected) in [
        (method::SET_WINDOW_OFFSET_X, 0),
        (method::SET_WINDOW_OFFSET_Y, 0),
        (method::SET_WINDOW_ORIGIN, 0),
        (method::SET_VIEWPORT_SCALE_OFFSET, 1),
        // Zero-to-one Z, no depth clamp, full XYZ frustum clipping.
        (method::SET_VIEWPORT_CLIP_CONTROL, 1 | (3 << 11)),
    ] {
        if *state.raw_registers.get(&register)? != expected {
            return None;
        }
    }
    let base = method::VIEWPORT_BASE + index as u16 * method::VIEWPORT_STRIDE;
    if *state.raw_registers.get(&(base + 6))? != 0x6420
        || *state.raw_registers.get(&(base + 7))? != 0
    {
        return None;
    }
    let mut values = [0.0; 6];
    for (field, value) in values.iter_mut().enumerate() {
        *value = f32::from_bits(*state.raw_registers.get(&(base + field as u16))?);
        if !value.is_finite() {
            return None;
        }
    }
    let [sx, sy, sz, ox, oy, oz] = values;
    // Vulkan maps X/Y from [-1,1], but Z from [0,1] in this clip convention.
    let viewport = crate::resource::ViewportDesc {
        x: ox - sx,
        y: oy - sy,
        width: 2.0 * sx,
        height: 2.0 * sy,
        min_depth: oz,
        max_depth: oz + sz,
    };
    if ![
        viewport.x,
        viewport.y,
        viewport.width,
        viewport.height,
        viewport.max_depth,
    ]
    .into_iter()
    .all(f32::is_finite)
        || viewport.width <= 0.0
        || !(0.0..=1.0).contains(&viewport.min_depth)
        || !(0.0..=1.0).contains(&viewport.max_depth)
    {
        return None;
    }
    Some(viewport)
}

/// Decode the captured rectangle without supplying missing register defaults.
/// Coordinates remain in the guest scissor space; window-offset translation,
/// render-area intersection and disabled-scissor handling belong to assembly.
pub fn captured_scissor(
    state: &crate::maxwell_state::DrawStateSnapshot,
    index: usize,
) -> Option<CapturedScissor> {
    use crate::maxwell_state::method;
    if index >= method::VIEWPORT_COUNT {
        return None;
    }
    let base = method::SCISSOR_BASE + index as u16 * method::SCISSOR_STRIDE;
    match *state.raw_registers.get(&base)? {
        0 => Some(CapturedScissor::Disabled),
        1 => {
            let horizontal = *state.raw_registers.get(&(base + 1))?;
            let vertical = *state.raw_registers.get(&(base + 2))?;
            let x = horizontal & 0xffff;
            let y = vertical & 0xffff;
            Some(CapturedScissor::Enabled(crate::resource::ScissorDesc {
                x: x as i32,
                y: y as i32,
                width: (horizontal >> 16).checked_sub(x)?,
                height: (vertical >> 16).checked_sub(y)?,
            }))
        }
        _ => None,
    }
}

/// Prepare dynamic state for one render area in the supported viewport convention.
/// Disabled guest scissoring means the whole explicit render area, not an empty
/// rectangle or an implicit framebuffer size. Empty intersections remain empty.
pub fn prepare_raster_area(
    state: &crate::maxwell_state::DrawStateSnapshot,
    index: usize,
    width: u32,
    height: u32,
) -> Option<[crate::Command; 2]> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    // This also validates zero window offsets and the coordinate convention
    // shared by the viewport and scissor. No independent scissor conversion may
    // silently admit a window origin unsupported by this assembly path.
    let viewport = captured_viewport(state, index)?;
    let scissor = match captured_scissor(state, index)? {
        CapturedScissor::Disabled => crate::resource::ScissorDesc {
            x: 0,
            y: 0,
            width,
            height,
        },
        CapturedScissor::Enabled(rectangle) => {
            let x = (rectangle.x as u32).min(width);
            let y = (rectangle.y as u32).min(height);
            let right = (rectangle.x as u32)
                .checked_add(rectangle.width)?
                .min(width);
            let bottom = (rectangle.y as u32)
                .checked_add(rectangle.height)?
                .min(height);
            crate::resource::ScissorDesc {
                x: x as i32,
                y: y as i32,
                width: right.checked_sub(x)?,
                height: bottom.checked_sub(y)?,
            }
        }
    };
    Some([
        crate::Command::SetViewport { viewport },
        crate::Command::SetScissor { scissor },
    ])
}

/// Validated non-indexed fetches and draw parameters. This is not a complete
/// render module: shader and fixed-function state must still be prepared.
#[derive(Debug, Clone)]
pub struct PreparedVertexArrayDraw {
    pub topology: PrimitiveTopology,
    pub vertex_input: crate::resource::VertexInputState,
    pub bindings: Vec<crate::Command>,
    pub draw: crate::Command,
    pub vertex_ranges: Vec<crate::address_space::ResolvedGpuRange>,
}

/// Select real allocation descriptors for prepared vertex ranges. Descriptor
/// sizes are never inferred from a mapped subrange or its guest address.
pub fn vertex_draw_buffers(
    draw: &PreparedVertexArrayDraw,
    allocations: &[crate::resource::BufferDesc],
) -> Option<Vec<crate::resource::BufferDesc>> {
    vertex_range_buffers(&draw.vertex_ranges, allocations)
}

/// Validate selected vertex ranges independently of indexed/non-indexed commands.
pub fn vertex_range_buffers(
    ranges: &[crate::address_space::ResolvedGpuRange],
    allocations: &[crate::resource::BufferDesc],
) -> Option<Vec<crate::resource::BufferDesc>> {
    use std::collections::BTreeMap;
    let mut available = BTreeMap::new();
    for allocation in allocations {
        if available.insert(allocation.id, allocation).is_some() {
            return None;
        }
    }
    let mut selected = BTreeMap::new();
    for range in ranges {
        let allocation = *available.get(&range.resource)?;
        if !allocation.usage.supports_vertex()
            || !range.permissions.read
            || range.length == 0
            || range.offset.checked_add(range.length)? > allocation.size
        {
            return None;
        }
        selected.insert(allocation.id, allocation.clone());
    }
    Some(selected.into_values().collect())
}

#[derive(Debug)]
pub enum VertexArrayDrawError {
    UnsupportedEvent,
    UnsupportedPrimitive,
    Vertex(crate::maxwell_state::VertexStreamResolveError),
}

impl std::fmt::Display for VertexArrayDrawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEvent => f.write_str("expected a captured vertex-array draw"),
            Self::UnsupportedPrimitive => {
                f.write_str("unsupported or missing draw primitive state")
            }
            Self::Vertex(error) => write!(f, "draw vertex input: {error}"),
        }
    }
}

impl std::error::Error for VertexArrayDrawError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Vertex(error) => Some(error),
            _ => None,
        }
    }
}

/// Consume the event's immutable capture, not later channel register state.
/// Every declared shader input is checked against the full selected draw range.
pub fn prepare_vertex_array_draw(
    event: &crate::maxwell_state::MaxwellEvent,
    interfaces: &[crate::maxwell_vertex::VertexAttributeInterface],
    address_space: &crate::GpuAddressSpace,
) -> Result<PreparedVertexArrayDraw, VertexArrayDrawError> {
    use crate::maxwell_state::{MaxwellEvent, merge_vertex_inputs};
    let MaxwellEvent::DrawVertexArray {
        first,
        count,
        state,
    } = event
    else {
        return Err(VertexArrayDrawError::UnsupportedEvent);
    };
    // Continuation requires captured ordered history, not just a raw BEGIN.
    if state.draw_instance_id().is_none() {
        return Err(VertexArrayDrawError::UnsupportedPrimitive);
    }
    let topology = draw_topology(state).ok_or(VertexArrayDrawError::UnsupportedPrimitive)?;
    if matches!(topology, PrimitiveTopology::Unknown(_)) {
        return Err(VertexArrayDrawError::UnsupportedPrimitive);
    }
    let inputs = interfaces
        .iter()
        .map(|interface| {
            state.prepare_draw_vertex_range(
                *interface,
                u64::from(*first),
                u64::from(*count),
                address_space,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(VertexArrayDrawError::Vertex)?;
    let (vertex_input, bindings) =
        merge_vertex_inputs(&inputs).map_err(VertexArrayDrawError::Vertex)?;
    let vertex_ranges = inputs.iter().map(|input| input.range).collect();
    Ok(PreparedVertexArrayDraw {
        topology,
        vertex_input,
        bindings,
        draw: crate::Command::Draw {
            vertices: *count,
            instances: 1,
            first_vertex: *first,
        },
        vertex_ranges,
    })
}

pub fn color_target_descriptor(
    state: &Maxwell3dState,
    index: usize,
) -> Option<UnresolvedTextureDesc> {
    let t = state.draw_state().color_target(index)?;
    let layout = t.block_linear_layer_layout()?;
    let (address, _) = t.block_linear_layer_range()?;
    Some(UnresolvedTextureDesc {
        address: RawGpuAddress {
            a: (address >> 32) as u32,
            b: address as u32,
        },
        width: t.width,
        height: t.height,
        depth: 1,
        layers: 1,
        mip_levels: 1,
        format: t.canonical_format()?,
        layout: TextureLayout::BlockLinear {
            block_height_log2: layout.block_height_log2,
            block_depth_log2: layout.block_depth_log2,
            raw: t.memory_layout,
        },
    })
}

pub fn map_color_format(raw: u32) -> TextureFormat {
    // Only formats verified by the backend should gain a portable enum. The raw value remains
    // available via Unknown for all other render-target encodings.
    crate::maxwell_state::ColorTargetState {
        format: raw,
        ..Default::default()
    }
    .canonical_format()
    .unwrap_or(TextureFormat::Unknown(raw))
}

#[cfg(test)]
mod tests {
    #[test]
    fn vertex_allocation_bounds_include_resource_offset_and_preserve_size() {
        use crate::{
            ResourceId,
            address_space::{GpuMapPermissions, ResolvedGpuRange},
            resource::{BufferDesc, BufferUsage},
        };
        let range = ResolvedGpuRange {
            resource: ResourceId(9),
            offset: 128,
            length: 64,
            permissions: GpuMapPermissions::READ_ONLY,
        };
        let mut draw = PreparedVertexArrayDraw {
            topology: PrimitiveTopology::TriangleList,
            vertex_input: Default::default(),
            bindings: vec![],
            draw: crate::Command::Draw {
                vertices: 0,
                instances: 1,
                first_vertex: 0,
            },
            vertex_ranges: vec![range, range],
        };
        let allocation = BufferDesc {
            id: range.resource,
            size: 192,
            usage: BufferUsage::Vertex,
        };
        assert_eq!(
            vertex_draw_buffers(&draw, std::slice::from_ref(&allocation))
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&allocation)
        );
        assert!(
            vertex_draw_buffers(
                &draw,
                &[BufferDesc {
                    size: 191,
                    ..allocation.clone()
                }]
            )
            .is_none()
        );
        assert!(vertex_draw_buffers(&draw, &[allocation.clone(), allocation.clone()]).is_none());
        for invalid in [
            ResolvedGpuRange {
                offset: u64::MAX,
                ..range
            },
            ResolvedGpuRange { length: 0, ..range },
            ResolvedGpuRange {
                permissions: GpuMapPermissions {
                    read: false,
                    write: true,
                    execute: false,
                },
                ..range
            },
        ] {
            draw.vertex_ranges = vec![invalid];
            assert!(vertex_draw_buffers(&draw, std::slice::from_ref(&allocation)).is_none());
        }
    }

    #[test]
    fn color_selection_preserves_shader_order_and_requires_exact_capture() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(captured_color_targets(&state).is_none());
        state
            .raw_registers
            .insert(method::SET_CT_SELECT, 3 | (5 << 4) | (2 << 7) | (7 << 10));
        assert!(captured_color_targets(&state).is_none());
        state.raw_registers.insert(method::SET_CT_MRT_ENABLE, 1);
        assert_eq!(captured_color_targets(&state).unwrap().targets, [5, 2, 7]);
        state.raw_registers.insert(method::SET_CT_SELECT, 0);
        assert!(captured_color_targets(&state).unwrap().targets.is_empty());
        for raw in [9, 0x10000001] {
            state.raw_registers.insert(method::SET_CT_SELECT, raw);
            assert!(captured_color_targets(&state).is_none());
        }
        state.raw_registers.insert(method::SET_CT_SELECT, 1);
        state.raw_registers.insert(method::SET_CT_MRT_ENABLE, 3);
        assert!(captured_color_targets(&state).is_none());
    }

    #[test]
    fn single_sample_capture_rejects_missing_state_and_preserves_mask() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(captured_single_sample(&state).is_none());
        for register in [
            method::SET_PS_OUTPUT_SAMPLE_MASK_USAGE,
            method::SET_ANTI_ALIAS_ENABLE,
            method::SET_ANTI_ALIAS_ALPHA_CONTROL,
            method::SET_ANTI_ALIAS,
            method::SET_ALPHA_TO_COVERAGE_OVERRIDE,
            method::SET_SAMPLE_MASK,
        ] {
            state.raw_registers.insert(register, 0);
        }
        assert_eq!(
            captured_single_sample(&state).unwrap().sample_mask,
            u32::MAX
        );
        for register in state.raw_registers.keys().copied().collect::<Vec<_>>() {
            let mut invalid = state.clone();
            invalid.raw_registers.remove(&register);
            assert!(captured_single_sample(&invalid).is_none());
        }
        state.raw_registers.insert(method::SET_SAMPLE_MASK, 0x10);
        assert!(captured_single_sample(&state).is_none());
        for mask in [0, 1, 0xffff] {
            state
                .raw_registers
                .insert(method::SET_COLOR_TARGET_SAMPLE_MASK, mask);
            assert_eq!(captured_single_sample(&state).unwrap().sample_mask, mask);
        }
        state
            .raw_registers
            .insert(method::SET_COLOR_TARGET_SAMPLE_MASK, 0x10000);
        assert!(captured_single_sample(&state).is_none());
        state.raw_registers.insert(method::SET_SAMPLE_MASK, 1);
        assert!(captured_single_sample(&state).is_none());
    }

    #[test]
    fn blend_encodings_cover_standard_factors_and_equations_without_masking() {
        use crate::resource::{BlendFactor, BlendOp};
        for (index, (ogl, factor)) in [
            (0x4000, BlendFactor::Zero),
            (0x4001, BlendFactor::One),
            (0x4300, BlendFactor::SrcColor),
            (0x4301, BlendFactor::OneMinusSrcColor),
            (0x4302, BlendFactor::SrcAlpha),
            (0x4303, BlendFactor::OneMinusSrcAlpha),
            (0x4304, BlendFactor::DstAlpha),
            (0x4305, BlendFactor::OneMinusDstAlpha),
            (0x4306, BlendFactor::DstColor),
            (0x4307, BlendFactor::OneMinusDstColor),
            (0x4308, BlendFactor::SrcAlphaSaturate),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(blend_factor(ogl), Some(factor));
            assert_eq!(blend_factor(index as u32 + 1), Some(factor));
            assert_eq!(blend_factor(ogl | 0x10000), None);
        }
        for (index, (ogl, op)) in [
            (0x8006, BlendOp::Add),
            (0x800a, BlendOp::Subtract),
            (0x800b, BlendOp::ReverseSubtract),
            (0x8007, BlendOp::Min),
            (0x8008, BlendOp::Max),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(blend_op(ogl), Some(op));
            assert_eq!(blend_op(index as u32 + 1), Some(op));
            assert_eq!(blend_op(ogl | 0x10000), None);
        }
        for raw in [0, 12, 13, 14, 15, 0xc001, 0xc900] {
            assert_eq!(blend_factor(raw), None);
        }
    }

    #[test]
    fn enabled_blend_requires_shared_equations_and_known_factors() {
        use crate::maxwell_state::DrawStateSnapshot;
        let mut state = DrawStateSnapshot::default();
        for (register, value) in [
            (method::SET_SINGLE_ROP_CONTROL, 0),
            (method::SET_LOGIC_OP, 0),
            (method::SET_BLEND_BASE, 1),
            (method::SET_CT_WRITE_BASE, 0x1111),
            (method::SET_BLEND_STATE_PER_TARGET, 0),
            (method::SET_BLEND_COLOR_OP, 0x800b),
            (method::SET_BLEND_ALPHA_OP, 3),
            (method::SET_BLEND_COLOR_SOURCE_COEFF, 0x4001),
            (method::SET_BLEND_COLOR_DEST_COEFF, 2),
            (method::SET_BLEND_ALPHA_SOURCE_COEFF, 2),
            (method::SET_BLEND_ALPHA_DEST_COEFF, 0x4001),
        ] {
            state.raw_registers.insert(register, value);
        }
        let target = captured_blend_target(&state, 0).unwrap();
        assert!(target.enable);
        assert_eq!(target.color_op, crate::resource::BlendOp::ReverseSubtract);
        assert!(captured_unblended_target(&state, 0).is_none());
        for register in state.raw_registers.keys().copied().collect::<Vec<_>>() {
            let mut invalid = state.clone();
            invalid.raw_registers.remove(&register);
            assert!(captured_blend_target(&invalid, 0).is_none());
        }
        for (register, value) in [
            (method::SET_BLEND_STATE_PER_TARGET, 1),
            (method::SET_BLEND_COLOR_SOURCE_COEFF, 0xc900),
            (method::SET_BLEND_COLOR_OP, 0x18006),
        ] {
            let mut invalid = state.clone();
            invalid.raw_registers.insert(register, value);
            assert!(captured_blend_target(&invalid, 0).is_none());
        }
    }

    #[test]
    fn unblended_target_requires_explicit_controls_and_sparse_mask_bits() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(captured_unblended_target(&state, 0).is_none());
        for register in [
            method::SET_SINGLE_ROP_CONTROL,
            method::SET_LOGIC_OP,
            method::SET_BLEND_BASE,
        ] {
            state.raw_registers.insert(register, 0);
        }
        for mask in 0..16u32 {
            let raw = (mask & 1) | ((mask & 2) << 3) | ((mask & 4) << 6) | ((mask & 8) << 9);
            state.raw_registers.insert(method::SET_CT_WRITE_BASE, raw);
            assert_eq!(
                captured_unblended_target(&state, 0)
                    .unwrap()
                    .color_write_mask,
                mask as u8
            );
        }
        state
            .raw_registers
            .insert(method::SET_CT_WRITE_BASE, 0x1113);
        assert!(captured_unblended_target(&state, 0).is_none());
        assert!(captured_unblended_target(&state, 8).is_none());
    }

    #[test]
    fn depth_capture_requires_controls_and_keeps_compare_encodings_exact() {
        use crate::resource::CompareOp;
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert!(captured_depth_stencil(&state).is_none());
        for register in [
            method::SET_DEPTH_TEST,
            method::SET_DEPTH_WRITE,
            method::SET_STENCIL_TEST,
        ] {
            state.raw_registers.insert(register, 0);
        }
        assert!(!captured_depth_stencil(&state).unwrap().depth_test);
        state.raw_registers.insert(method::SET_DEPTH_TEST, 1);
        assert!(captured_depth_stencil(&state).is_none());
        state.raw_registers.insert(method::SET_DEPTH_WRITE, 1);
        for (index, compare) in [
            CompareOp::Never,
            CompareOp::Less,
            CompareOp::Equal,
            CompareOp::LessOrEqual,
            CompareOp::Greater,
            CompareOp::NotEqual,
            CompareOp::GreaterOrEqual,
            CompareOp::Always,
        ]
        .into_iter()
        .enumerate()
        {
            for raw in [index as u32 + 1, index as u32 + 0x200] {
                state.raw_registers.insert(method::SET_DEPTH_FUNC, raw);
                let depth = captured_depth_stencil(&state).unwrap();
                assert_eq!(depth.depth_compare, compare);
                assert!(depth.depth_write);
            }
        }
        for raw in [0, 9, 0x208, 0x10201] {
            state.raw_registers.insert(method::SET_DEPTH_FUNC, raw);
            assert!(captured_depth_stencil(&state).is_none());
        }
        state.raw_registers.insert(method::SET_DEPTH_TEST, 0);
        assert!(captured_depth_stencil(&state).is_some());
        for (register, raw) in [
            (method::SET_STENCIL_TEST, 1),
            (method::SET_DEPTH_TEST, 3),
            (method::SET_DEPTH_WRITE, 3),
        ] {
            let mut invalid = state.clone();
            invalid.raw_registers.insert(register, raw);
            assert!(captured_depth_stencil(&invalid).is_none());
        }
    }

    #[test]
    fn depth_bias_capture_checks_words_and_hardware_units() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        for (register, value) in [
            (method::SET_DEPTH_BIAS_CONTROL, 1),
            (method::SET_DEPTH_BIAS, (-6f32).to_bits()),
            (method::SET_SLOPE_SCALE_DEPTH_BIAS, 1.5f32.to_bits()),
            (method::SET_DEPTH_BIAS_CLAMP, 0),
        ] {
            state.raw_registers.insert(register, value);
        }
        assert_eq!(
            captured_depth_bias(&state).unwrap().constant_bits,
            (-3f32).to_bits()
        );
        for register in state.raw_registers.keys().copied().collect::<Vec<_>>() {
            let mut invalid = state.clone();
            invalid.raw_registers.remove(&register);
            assert_eq!(captured_depth_bias(&invalid), None);
        }
        for (register, value) in [
            (method::SET_DEPTH_BIAS_CONTROL, 0),
            (method::SET_DEPTH_BIAS_CONTROL, 3),
            (method::SET_DEPTH_BIAS, f32::INFINITY.to_bits()),
            (method::SET_SLOPE_SCALE_DEPTH_BIAS, f32::NAN.to_bits()),
            (method::SET_DEPTH_BIAS_CLAMP, f32::NEG_INFINITY.to_bits()),
        ] {
            let mut invalid = state.clone();
            invalid.raw_registers.insert(register, value);
            assert_eq!(captured_depth_bias(&invalid), None);
        }
    }

    #[test]
    fn captured_faces_reject_missing_or_incompatible_state() {
        use crate::resource::{CullMode, FrontFace, PolygonMode};
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert_eq!(captured_faces(&state), None);
        for (register, value) in [
            (method::OGL_SET_CULL, 1),
            (method::OGL_SET_CULL_FACE, 0x405),
            (method::OGL_SET_FRONT_FACE, 0x901),
            (method::SET_FRONT_POLYGON_MODE, 0x1b02),
            (method::SET_BACK_POLYGON_MODE, 0x1b02),
        ] {
            state.raw_registers.insert(register, value);
        }
        assert_eq!(
            captured_faces(&state),
            Some(CapturedFaces {
                cull: CullMode::Back,
                front_face: FrontFace::CounterClockwise,
                polygon_mode: PolygonMode::Fill,
            })
        );
        for register in state.raw_registers.keys().copied().collect::<Vec<_>>() {
            let mut incomplete = state.clone();
            incomplete.raw_registers.remove(&register);
            assert_eq!(captured_faces(&incomplete), None);
        }
        for (register, value) in [
            (method::OGL_SET_CULL, 3),
            (method::OGL_SET_CULL_FACE, 0),
            (method::OGL_SET_FRONT_FACE, 0),
            (method::SET_BACK_POLYGON_MODE, 0x1b01),
        ] {
            let mut invalid = state.clone();
            invalid.raw_registers.insert(register, value);
            assert_eq!(captured_faces(&invalid), None);
        }
        state.raw_registers.insert(method::OGL_SET_CULL, 0);
        state.raw_registers.remove(&method::OGL_SET_CULL_FACE);
        assert_eq!(captured_faces(&state).unwrap().cull, CullMode::None);
        assert!(captured_raster(&state).is_none());
        state.raw_registers.insert(method::SET_RASTER_ENABLE, 1);
        state
            .raw_registers
            .insert(method::SET_VIEWPORT_CLIP_CONTROL, 0x1801);
        state.raw_registers.insert(method::SET_POLY_OFFSET_FILL, 0);
        assert!(!captured_raster(&state).unwrap().rasterizer_discard);
        state.raw_registers.insert(method::SET_RASTER_ENABLE, 0);
        assert!(captured_raster(&state).unwrap().rasterizer_discard);
        for (register, value) in [
            (method::SET_RASTER_ENABLE, 3),
            (method::SET_POLY_OFFSET_FILL, 1),
            (method::SET_VIEWPORT_CLIP_CONTROL, 0x1819),
        ] {
            let mut invalid = state.clone();
            invalid.raw_registers.insert(register, value);
            assert!(captured_raster(&invalid).is_none());
        }
    }

    #[test]
    fn viewport_capture_checks_complete_transform_and_explicit_conventions() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert_eq!(captured_viewport(&state, 0), None);
        for (register, value) in [
            (method::SET_WINDOW_OFFSET_X, 0),
            (method::SET_WINDOW_OFFSET_Y, 0),
            (method::SET_WINDOW_ORIGIN, 0),
            (method::SET_VIEWPORT_SCALE_OFFSET, 1),
            (method::SET_VIEWPORT_CLIP_CONTROL, 0x1801),
            (method::VIEWPORT_BASE + 6, 0x6420),
            (method::VIEWPORT_BASE + 7, 0),
        ] {
            state.raw_registers.insert(register, value);
        }
        for (field, value) in [16.25f32, -16.0, -0.5, 16.75, 32.25, 0.75]
            .into_iter()
            .enumerate()
        {
            state
                .raw_registers
                .insert(method::VIEWPORT_BASE + field as u16, value.to_bits());
        }
        assert_eq!(
            captured_viewport(&state, 0),
            Some(crate::resource::ViewportDesc {
                x: 0.5,
                y: 48.25,
                width: 32.5,
                height: -32.0,
                min_depth: 0.75,
                max_depth: 0.25,
            })
        );
        for register in state.raw_registers.keys().copied().collect::<Vec<_>>() {
            let mut incomplete = state.clone();
            incomplete.raw_registers.remove(&register);
            assert_eq!(captured_viewport(&incomplete, 0), None);
        }
        for (register, value) in [
            (method::SET_VIEWPORT_CLIP_CONTROL, 0x1800),
            (method::SET_VIEWPORT_CLIP_CONTROL, 0x1819),
            (method::SET_WINDOW_ORIGIN, 1),
            (method::SET_WINDOW_OFFSET_X, 1),
            (method::SET_VIEWPORT_SCALE_OFFSET, 0),
            (method::VIEWPORT_BASE + 6, 0),
            (method::VIEWPORT_BASE + 7, 1),
            (method::VIEWPORT_BASE, (-1f32).to_bits()),
            (method::VIEWPORT_BASE + 1, f32::NAN.to_bits()),
            (method::VIEWPORT_BASE + 2, 1f32.to_bits()),
        ] {
            let mut unsupported = state.clone();
            unsupported.raw_registers.insert(register, value);
            assert_eq!(captured_viewport(&unsupported, 0), None);
        }
        assert_eq!(captured_viewport(&state, method::VIEWPORT_COUNT), None);
        assert!(prepare_raster_area(&state, 0, 64, 64).is_none());
        state.raw_registers.insert(method::SCISSOR_BASE, 0);
        let [_, crate::Command::SetScissor { scissor }] =
            prepare_raster_area(&state, 0, 64, 32).unwrap()
        else {
            panic!("expected viewport and scissor commands")
        };
        assert_eq!(
            scissor,
            crate::resource::ScissorDesc {
                x: 0,
                y: 0,
                width: 64,
                height: 32
            }
        );
        state.raw_registers.insert(method::SCISSOR_BASE, 1);
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 8 | (100 << 16));
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 2, 9 | (40 << 16));
        let [_, crate::Command::SetScissor { scissor }] =
            prepare_raster_area(&state, 0, 64, 32).unwrap()
        else {
            panic!("expected viewport and scissor commands")
        };
        assert_eq!(
            scissor,
            crate::resource::ScissorDesc {
                x: 8,
                y: 9,
                width: 56,
                height: 23
            }
        );
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 80 | (100 << 16));
        let [_, crate::Command::SetScissor { scissor }] =
            prepare_raster_area(&state, 0, 64, 32).unwrap()
        else {
            panic!("expected viewport and scissor commands")
        };
        assert_eq!(scissor.x, 64);
        assert_eq!(scissor.width, 0);
        for (width, height) in [(0, 32), (64, 0), (u32::MAX, 32), (64, u32::MAX)] {
            assert!(prepare_raster_area(&state, 0, width, height).is_none());
        }
    }

    #[test]
    fn scissor_capture_requires_enabled_bounds_and_preserves_empty_rectangles() {
        let mut state = crate::maxwell_state::DrawStateSnapshot::default();
        assert_eq!(captured_scissor(&state, 0), None);
        state.raw_registers.insert(method::SCISSOR_BASE, 0);
        assert_eq!(captured_scissor(&state, 0), Some(CapturedScissor::Disabled));
        state.raw_registers.insert(method::SCISSOR_BASE, 1);
        assert_eq!(captured_scissor(&state, 0), None);
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 7 | (31 << 16));
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 2, 9 | (29 << 16));
        assert_eq!(
            captured_scissor(&state, 0),
            Some(CapturedScissor::Enabled(crate::resource::ScissorDesc {
                x: 7,
                y: 9,
                width: 24,
                height: 20
            }))
        );
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 7 | (7 << 16));
        assert!(matches!(
            captured_scissor(&state, 0),
            Some(CapturedScissor::Enabled(crate::resource::ScissorDesc {
                width: 0,
                ..
            }))
        ));
        state
            .raw_registers
            .insert(method::SCISSOR_BASE + 1, 8 | (7 << 16));
        assert_eq!(captured_scissor(&state, 0), None);
        state.raw_registers.insert(method::SCISSOR_BASE, 3);
        assert_eq!(captured_scissor(&state, 0), None);
        assert_eq!(captured_scissor(&state, method::VIEWPORT_COUNT), None);
    }

    #[test]
    fn vertex_array_draw_preserves_selection_and_rejects_continuation() {
        use crate::maxwell_state::{DrawStateSnapshot, MaxwellEvent};
        let mut state = DrawStateSnapshot {
            primitive_raw: Some(4),
            ..Default::default()
        };
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 0);
        let event = |state| MaxwellEvent::DrawVertexArray {
            first: 9,
            count: 7,
            state: Box::new(state),
        };
        let space = crate::GpuAddressSpace::new();
        let prepared = prepare_vertex_array_draw(&event(state.clone()), &[], &space).unwrap();
        assert_eq!(prepared.topology, PrimitiveTopology::TriangleList);
        let mut continued = state.clone();
        continued.primitive_raw = Some(4 | method::BEGIN_INSTANCE_CONTINUE);
        continued.instance_id = Some(3);
        assert_eq!(
            prepare_vertex_array_draw(&event(continued.clone()), &[], &space)
                .unwrap()
                .topology,
            PrimitiveTopology::TriangleList
        );
        continued.instance_id = None;
        assert!(prepare_vertex_array_draw(&event(continued), &[], &space).is_err());
        assert!(matches!(
            prepared.draw,
            crate::Command::Draw {
                first_vertex: 9,
                vertices: 7,
                instances: 1,
            }
        ));
        assert!(prepared.bindings.is_empty());
        assert!(matches!(
            prepare_vertex_array_draw(&MaxwellEvent::End, &[], &space),
            Err(VertexArrayDrawError::UnsupportedEvent)
        ));
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 1);
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY, 4);
        state.primitive_raw = Some(4 | (1 << 26));
        assert!(matches!(
            prepare_vertex_array_draw(&event(state), &[], &space),
            Err(VertexArrayDrawError::UnsupportedPrimitive)
        ));
    }

    #[test]
    fn topology_control_keeps_begin_and_separate_encodings_distinct() {
        use crate::maxwell_state::{DrawStateSnapshot, method};
        let mut state = DrawStateSnapshot {
            primitive_raw: Some(1),
            ..Default::default()
        };
        assert_eq!(super::draw_topology(&state), None);
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 0);
        assert_eq!(
            super::draw_topology(&state),
            Some(super::PrimitiveTopology::LineList)
        );
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY_CONTROL, 1);
        assert_eq!(super::draw_topology(&state), None);
        state
            .raw_registers
            .insert(method::SET_PRIMITIVE_TOPOLOGY, 1);
        assert_eq!(
            super::draw_topology(&state),
            Some(super::PrimitiveTopology::PointList)
        );
        for raw in [0, 6, 0x1004, 0x10000] {
            assert_eq!(super::topology(raw), super::PrimitiveTopology::Unknown(raw));
        }
        assert_eq!(
            super::begin_topology(2),
            super::PrimitiveTopology::Unknown(2)
        );
        assert_eq!(
            super::begin_topology(4 | (1 << 26)),
            super::PrimitiveTopology::Unknown(4 | (1 << 26))
        );
    }
    use super::*;
    use crate::maxwell_state::method;

    #[test]
    fn color_descriptor_uses_complete_capture_and_selected_layer() {
        let mut state = Maxwell3dState::default();
        assert!(color_target_descriptor(&state, 0).is_none());
        for (field, word) in [0, 0x2000, 65, 9, 0xe5, 0x20, 2, 2560, 1]
            .into_iter()
            .enumerate()
        {
            state
                .raw_registers
                .insert(method::COLOR_TARGET_BASE + field as u16, word);
        }
        let desc = color_target_descriptor(&state, 0).unwrap();
        assert_eq!(desc.address.high_low(), 0x4800);
        assert_eq!(desc.layers, 1);
        assert_eq!(desc.format, TextureFormat::R32Float);
        assert_eq!(
            desc.layout,
            TextureLayout::BlockLinear {
                block_height_log2: 2,
                block_depth_log2: 0,
                raw: 0x20
            }
        );
        state
            .raw_registers
            .insert(method::COLOR_TARGET_BASE + 5, 0x1000);
        assert!(color_target_descriptor(&state, 0).is_none());
        assert_eq!(map_color_format(0xd5), TextureFormat::Rgba8Unorm);
        assert_eq!(map_color_format(0x1d5), TextureFormat::Unknown(0x1d5));
    }
}
