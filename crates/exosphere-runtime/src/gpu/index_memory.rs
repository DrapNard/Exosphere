// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated index-buffer reads through canonical live guest GPU mappings.

use exosphere_gpu_ir::maxwell_state::IndexBufferState;
use exosphere_horizon::{
    HorizonError,
    memory::{AddressSpace, MemoryPermission},
    nvdrv::address_space::NvGpuAddressSpace,
};

/// Software allocation budget, not a guest hardware capability.
const MAX_INDEX_READ_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IndexReadError {
    #[error("unsupported index format or invalid GPU address range")]
    InvalidState,
    #[error("index range exceeds the programmed inclusive buffer limit")]
    LimitExceeded,
    #[error("index read exceeds the software allocation budget")]
    Budget,
    #[error("index plus base vertex selects a negative or overflowing element")]
    InvalidVertex,
    #[error(transparent)]
    Vertex(#[from] exosphere_gpu_ir::maxwell_state::VertexStreamResolveError),
    #[error(transparent)]
    Memory(#[from] HorizonError),
    #[error(transparent)]
    Mapping(#[from] exosphere_gpu_ir::address_space::GpuAddressError),
}

#[derive(Debug, Clone, Copy)]
pub struct IndexedVertexSelection {
    pub indices: IndexBufferState,
    pub count: u32,
    pub base_vertex: i32,
    pub restart_index: Option<u32>,
}

#[derive(Debug)]
pub struct CapturedIndexedVertexInputs {
    pub selection: IndexedVertexSelection,
    pub inputs: Vec<exosphere_gpu_ir::maxwell_state::PreparedVertexInput>,
    pub index_range: Option<exosphere_gpu_ir::address_space::ResolvedGpuRange>,
    pub index_binding: Option<exosphere_gpu_ir::Command>,
}

impl CapturedIndexedVertexInputs {
    /// Assemble native per-vertex fetch commands without applying FIRST twice.
    /// Only the native index type's standard restart marker is accepted here;
    /// module assembly must also validate a restart-capable topology.
    /// Allocation selection must still revalidate these public prepared fields
    /// against the live resource snapshot before module submission.
    pub fn draw_commands(&self) -> Result<Vec<exosphere_gpu_ir::Command>, IndexReadError> {
        use exosphere_gpu_ir::Command;
        let marker = match self.selection.indices.index_size_raw {
            0 => 0xff,
            1 => 0xffff,
            2 => u32::MAX,
            _ => return Err(IndexReadError::InvalidState),
        };
        if self
            .selection
            .restart_index
            .is_some_and(|value| value != marker)
        {
            return Err(IndexReadError::InvalidState);
        }
        let (_, mut commands) = exosphere_gpu_ir::maxwell_state::merge_vertex_inputs(&self.inputs)?;
        match (&self.index_binding, self.index_range) {
            (Some(binding @ Command::BindIndexBuffer { buffer, offset, .. }), Some(range))
                if self.selection.count != 0
                    && *buffer == range.resource
                    && *offset == range.offset =>
            {
                commands.push(binding.clone());
            }
            (None, None) if self.selection.count == 0 => return Ok(Vec::new()),
            _ => return Err(IndexReadError::InvalidState),
        }
        commands.push(Command::DrawIndexed {
            indices: self.selection.count,
            instances: 1,
            first_index: 0,
            vertex_offset: self.selection.base_vertex,
        });
        Ok(commands)
    }
}

/// Prepare per-vertex fetches from the draw's own index descriptor, count, signed
/// element base and restart state. Required controls must have been captured;
/// absent words do not silently become guessed reset values. This does not
/// prepare instance inputs, VertexID semantics, a pipeline or draw submission.
pub fn prepare_captured_indexed_vertex_inputs(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    event: &exosphere_gpu_ir::MaxwellEvent,
    interfaces: &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface],
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<CapturedIndexedVertexInputs, IndexReadError> {
    use exosphere_gpu_ir::maxwell_state::{MaxwellEvent, method};
    let MaxwellEvent::DrawIndexBuffer { raw, state } = event else {
        return Err(IndexReadError::InvalidState);
    };
    let captured = |register| {
        state
            .raw_registers
            .get(&register)
            .copied()
            .ok_or(IndexReadError::InvalidState)
    };
    let base_vertex = captured(method::SET_VERTEX_ELEMENT_BASE)? as i32;
    let restart_index = match captured(method::SET_PRIMITIVE_RESTART)? {
        0 => None,
        1 => Some(captured(method::SET_PRIMITIVE_RESTART_INDEX)?),
        _ => return Err(IndexReadError::InvalidState),
    };
    // Nouveau emits INDEX_BATCH_FIRST followed by the complete u32 count.
    let selection = IndexedVertexSelection {
        indices: state.index_buffer,
        count: *raw,
        base_vertex,
        restart_index,
    };
    let inputs =
        prepare_indexed_vertex_inputs(memory, gpu, selection, state, interfaces, resources)?;
    let index_range = match selection.indices.index_read_range(selection.count) {
        None if selection.count == 0 => None,
        None => return Err(IndexReadError::InvalidState),
        Some((address, bytes)) => {
            let range = resources.resolve(address, bytes)?;
            let width = selection
                .indices
                .index_bytes()
                .ok_or(IndexReadError::InvalidState)?;
            if !range.permissions.read || range.offset % width != 0 {
                return Err(IndexReadError::InvalidState);
            }
            Some(range)
        }
    };
    let index_binding = index_range.map(|range| exosphere_gpu_ir::Command::BindIndexBuffer {
        buffer: range.resource,
        // FIRST is already incorporated in index_read_range. A later native
        // DrawIndexed must not add it a second time through first_index.
        offset: range.offset,
        index_type: match selection.indices.index_size_raw {
            0 => exosphere_gpu_ir::resource::IndexType::U8,
            1 => exosphere_gpu_ir::resource::IndexType::U16,
            2 => exosphere_gpu_ir::resource::IndexType::U32,
            _ => unreachable!("nonempty indices were validated before binding"),
        },
    });
    Ok(CapturedIndexedVertexInputs {
        selection,
        inputs,
        index_range,
        index_binding,
    })
}

/// Prepare per-vertex inputs from actual indices. Instance inputs require their
/// separately resolved instance/divisor selection and are not included here.
pub fn prepare_indexed_vertex_inputs(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    selection: IndexedVertexSelection,
    snapshot: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot,
    interfaces: &[exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface],
    resources: &exosphere_gpu_ir::GpuAddressSpace,
) -> Result<Vec<exosphere_gpu_ir::maxwell_state::PreparedVertexInput>, IndexReadError> {
    let bounds = read_indexed_vertex_bounds(
        memory,
        gpu,
        selection.indices,
        selection.count,
        selection.base_vertex,
        selection.restart_index,
    )?;
    let (first, count) = match bounds {
        None => (0, 0),
        Some((minimum, maximum)) => (minimum, maximum - minimum + 1),
    };
    interfaces
        .iter()
        .map(|interface| {
            snapshot
                .prepare_draw_vertex_range(*interface, first, count, resources)
                .map_err(IndexReadError::Vertex)
        })
        .collect()
}

/// Read live indices and bound the effective vertex elements. Restart markers
/// are skipped only when an explicit marker is supplied; zero/all-restart draws have no
/// vertex fetches. Returned bounds do not substitute for stream validation.
pub fn read_indexed_vertex_bounds(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    state: IndexBufferState,
    count: u32,
    base_vertex: i32,
    restart_index: Option<u32>,
) -> Result<Option<(u64, u64)>, IndexReadError> {
    if count == 0 {
        return Ok(None);
    }
    let width = state.index_bytes().ok_or(IndexReadError::InvalidState)? as usize;
    let maximum_index = match width {
        1 => u8::MAX as u32,
        2 => u16::MAX as u32,
        4 => u32::MAX,
        _ => return Err(IndexReadError::InvalidState),
    };
    if restart_index.is_some_and(|restart| restart > maximum_index) {
        return Err(IndexReadError::InvalidState);
    }
    // Validate restart metadata before allocating or reading guest index bytes.
    let bytes = read_indices(memory, gpu, state, count)?;
    let mut bounds: Option<(u64, u64)> = None;
    for encoded in bytes.chunks_exact(width) {
        let index = encoded
            .iter()
            .enumerate()
            .fold(0u32, |value, (byte, bits)| {
                value | (u32::from(*bits) << (byte * 8))
            });
        if restart_index == Some(index) {
            continue;
        }
        let element = u64::from(index)
            .checked_add_signed(i64::from(base_vertex))
            .ok_or(IndexReadError::InvalidVertex)?;
        bounds = Some(match bounds {
            None => (element, element),
            Some((minimum, maximum)) => (minimum.min(element), maximum.max(element)),
        });
    }
    Ok(bounds)
}

/// Return exact encoded indices, without conversion or primitive-restart handling.
/// This is not a Vulkan buffer binding or evidence that a draw executed.
pub fn read_indices(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    state: IndexBufferState,
    count: u32,
) -> Result<Vec<u8>, IndexReadError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let (address, bytes) = state
        .index_read_range(count)
        .ok_or(IndexReadError::InvalidState)?;
    if state.limit.a & !0xff != 0 || state.limit.high_low() < state.address.high_low() {
        return Err(IndexReadError::InvalidState);
    }
    // Nouveau programs this register as buffer address + byte size - 1.
    if address + bytes - 1 > state.limit.high_low() {
        return Err(IndexReadError::LimitExceeded);
    }
    if bytes > MAX_INDEX_READ_BYTES {
        return Err(IndexReadError::Budget);
    }
    let size = usize::try_from(bytes).map_err(|_| IndexReadError::Budget)?;
    gpu.validate_access(memory, address, size, MemoryPermission::READ)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| IndexReadError::Budget)?;
    output.resize(size, 0);
    gpu.read(memory, address, &mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_gpu_ir::maxwell_state::RawGpuAddress;
    use exosphere_horizon::{
        memory::{MemoryKind, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn invalid_restart_metadata_rejects_before_index_memory_access() {
        let memory = AddressSpace::default();
        let gpu = NvGpuAddressSpace::default();
        let state = IndexBufferState {
            address: RawGpuAddress { a: 0, b: 0x100000 },
            limit: RawGpuAddress { a: 0, b: 0x100000 },
            index_size_raw: 0,
            first: 0,
        };
        assert!(matches!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 1, 0, Some(256)),
            Err(IndexReadError::InvalidState)
        ));
        assert!(matches!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 1, 0, Some(255)),
            Err(IndexReadError::Memory(_))
        ));
        assert_eq!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 0, 0, Some(256)).unwrap(),
            None
        );
    }

    #[test]
    fn reads_live_indices_and_checks_limit_before_memory_access() {
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
        let mut state = IndexBufferState {
            address: RawGpuAddress { a: 0, b: 0x100000 },
            limit: RawGpuAddress { a: 0, b: 0x10000f },
            index_size_raw: 1,
            first: 2,
        };
        for bytes in [[1, 2, 3, 4], [5, 6, 7, 8]] {
            memory.write(0x4004, &bytes).unwrap();
            assert_eq!(read_indices(&memory, &gpu, state, 2).unwrap(), bytes);
        }
        assert!(read_indices(&memory, &gpu, state, 0).unwrap().is_empty());
        memory.write(0x4004, &[2, 0, 255, 255]).unwrap();
        assert_eq!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 2, -1, Some(65535)).unwrap(),
            Some((1, 1))
        );
        assert_eq!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 2, 3, None).unwrap(),
            Some((5, 65538))
        );
        memory.write(0x4004, &[255; 4]).unwrap();
        assert_eq!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 2, -1, Some(65535)).unwrap(),
            None
        );
        memory.write(0x4004, &[0; 4]).unwrap();
        assert!(matches!(
            read_indexed_vertex_bounds(&memory, &gpu, state, 2, -1, None),
            Err(IndexReadError::InvalidVertex)
        ));
        for (format, width, maximum) in [(0, 1usize, 255u32), (1, 2, 65535), (2, 4, u32::MAX)] {
            let mut sized = state;
            sized.index_size_raw = format;
            sized.first = 1;
            let mut bytes = Vec::new();
            for index in [3u32, maximum, 7] {
                bytes.extend_from_slice(&index.to_le_bytes()[..width]);
            }
            memory.write(0x4000 + width as u64, &bytes).unwrap();
            assert_eq!(
                read_indexed_vertex_bounds(&memory, &gpu, sized, 3, -2, Some(maximum)).unwrap(),
                Some((1, 5))
            );
            assert_eq!(
                read_indexed_vertex_bounds(&memory, &gpu, sized, 3, 0, Some(3)).unwrap(),
                Some((7, u64::from(maximum)))
            );
            assert_eq!(
                read_indexed_vertex_bounds(&memory, &gpu, sized, 3, 0, None).unwrap(),
                Some((3, u64::from(maximum)))
            );
            if maximum != u32::MAX {
                assert!(matches!(
                    read_indexed_vertex_bounds(&memory, &gpu, sized, 3, 0, Some(maximum + 1)),
                    Err(IndexReadError::InvalidState)
                ));
            }
            memory
                .write(0x4000 + width as u64, &vec![255; width * 3])
                .unwrap();
            assert_eq!(
                read_indexed_vertex_bounds(&memory, &gpu, sized, 3, i32::MIN, Some(maximum))
                    .unwrap(),
                None
            );
        }
        state.limit.b = 0x100007;
        let mut snapshot = exosphere_gpu_ir::maxwell_state::DrawStateSnapshot {
            primitive_raw: Some(4),
            ..Default::default()
        };
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE, 0);
        use exosphere_gpu_ir::maxwell_state::method;
        for (offset, word) in [0x1010, 0, 0x100000, 0].into_iter().enumerate() {
            snapshot
                .raw_registers
                .insert(method::VERTEX_STREAM_BASE + offset as u16, word);
        }
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE, 0);
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_LIMIT_BASE + 1, 0x10003f);
        snapshot
            .raw_registers
            .insert(method::VERTEX_ATTRIBUTE_BASE, (4 << 21) | (7 << 27));
        let interfaces = [exosphere_gpu_ir::maxwell_vertex::VertexAttributeInterface {
            slot: 0,
            location: 3,
            format: exosphere_gpu_ir::VertexFormat::Rg32Float,
        }];
        let mut resources = exosphere_gpu_ir::GpuAddressSpace::new();
        resources
            .map(exosphere_gpu_ir::GpuMapping {
                guest_base: 0x100000,
                size: 64,
                resource: exosphere_gpu_ir::ResourceId(8),
                resource_offset: 128,
                permissions: exosphere_gpu_ir::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        let selection = IndexedVertexSelection {
            indices: state,
            count: 2,
            base_vertex: 0,
            restart_index: None,
        };
        memory.write(0x4004, &[1, 0, 3, 0]).unwrap();
        let prepared = prepare_indexed_vertex_inputs(
            &memory,
            &gpu,
            selection,
            &snapshot,
            &interfaces,
            &resources,
        )
        .unwrap();
        assert_eq!(prepared[0].range.offset, 128);
        assert_eq!(prepared[0].attribute.location, 3);
        snapshot
            .raw_registers
            .remove(&method::VERTEX_STREAM_PER_INSTANCE_BASE);
        assert!(snapshot.captured_vertex_input_rate(interfaces[0]).is_err());
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE, 2);
        assert!(snapshot.captured_vertex_input_rate(interfaces[0]).is_err());
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE, 1);
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_BASE + 3, 3);
        let instanced = prepare_indexed_vertex_inputs(
            &memory,
            &gpu,
            selection,
            &snapshot,
            &interfaces,
            &resources,
        )
        .unwrap();
        assert_eq!(
            instanced[0].binding.input_rate,
            exosphere_gpu_ir::VertexInputRate::Instance
        );
        snapshot.primitive_raw = Some(4 | method::BEGIN_INSTANCE_CONTINUE);
        for (instance, offset) in [(1, 128), (2, 128), (3, 144), (11, 176)] {
            snapshot.instance_id = Some(instance);
            let selected = prepare_indexed_vertex_inputs(
                &memory,
                &gpu,
                selection,
                &snapshot,
                &interfaces,
                &resources,
            )
            .unwrap();
            assert_eq!(selected[0].range.offset, offset);
            assert_eq!(selected[0].range.offset + selected[0].range.length, 192);
        }
        snapshot.instance_id = Some(12);
        assert!(
            prepare_indexed_vertex_inputs(
                &memory,
                &gpu,
                selection,
                &snapshot,
                &interfaces,
                &resources
            )
            .is_err()
        );
        snapshot.primitive_raw = Some(4);
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_BASE + 3, 0);
        assert!(snapshot.captured_vertex_input_rate(interfaces[0]).is_err());
        snapshot
            .raw_registers
            .insert(method::VERTEX_STREAM_PER_INSTANCE_BASE, 0);
        snapshot.index_buffer = state;
        let event = |snapshot: &exosphere_gpu_ir::maxwell_state::DrawStateSnapshot| {
            exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
                raw: 2,
                state: Box::new(snapshot.clone()),
            }
        };
        assert!(matches!(
            prepare_captured_indexed_vertex_inputs(
                &memory,
                &gpu,
                &event(&snapshot),
                &interfaces,
                &resources
            ),
            Err(IndexReadError::InvalidState)
        ));
        snapshot
            .raw_registers
            .insert(method::SET_VERTEX_ELEMENT_BASE, 0);
        snapshot
            .raw_registers
            .insert(method::SET_PRIMITIVE_RESTART, 0);
        let mut captured = prepare_captured_indexed_vertex_inputs(
            &memory,
            &gpu,
            &event(&snapshot),
            &interfaces,
            &resources,
        )
        .unwrap();
        assert_eq!(captured.selection.count, 2);
        assert_eq!(captured.selection.base_vertex, 0);
        assert_eq!(captured.inputs[0].range, prepared[0].range);
        let range = captured.index_range.unwrap();
        assert_eq!((range.offset, range.length), (132, 4));
        let commands = captured.draw_commands().unwrap();
        assert!(matches!(
            commands.last(),
            Some(exosphere_gpu_ir::Command::DrawIndexed {
                indices: 2,
                instances: 1,
                first_index: 0,
                vertex_offset: 0,
            })
        ));
        assert_eq!(
            commands
                .iter()
                .filter(|command| matches!(
                    command,
                    exosphere_gpu_ir::Command::BindIndexBuffer { .. }
                ))
                .count(),
            1
        );
        captured.selection.restart_index = Some(7);
        assert!(matches!(
            captured.draw_commands(),
            Err(IndexReadError::InvalidState)
        ));
        captured.selection.restart_index = None;
        assert!(matches!(
            captured.index_binding,
            Some(exosphere_gpu_ir::Command::BindIndexBuffer {
                buffer: exosphere_gpu_ir::ResourceId(8),
                offset: 132,
                index_type: exosphere_gpu_ir::resource::IndexType::U16,
            })
        ));
        let empty = exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer {
            raw: 0,
            state: Box::new(snapshot.clone()),
        };
        let empty = prepare_captured_indexed_vertex_inputs(
            &memory,
            &NvGpuAddressSpace::default(),
            &empty,
            &interfaces,
            &resources,
        )
        .unwrap();
        assert!(empty.index_range.is_none() && empty.index_binding.is_none());
        let mut misaligned = exosphere_gpu_ir::GpuAddressSpace::new();
        misaligned
            .map(exosphere_gpu_ir::GpuMapping {
                guest_base: 0x100000,
                size: 64,
                resource: exosphere_gpu_ir::ResourceId(8),
                resource_offset: 129,
                permissions: exosphere_gpu_ir::GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        assert!(matches!(
            prepare_captured_indexed_vertex_inputs(
                &memory,
                &gpu,
                &event(&snapshot),
                &interfaces,
                &misaligned
            ),
            Err(IndexReadError::InvalidState)
        ));
        snapshot
            .raw_registers
            .insert(method::SET_VERTEX_ELEMENT_BASE, (-2i32) as u32);
        assert!(matches!(
            prepare_captured_indexed_vertex_inputs(
                &memory,
                &gpu,
                &event(&snapshot),
                &interfaces,
                &resources
            ),
            Err(IndexReadError::InvalidVertex)
        ));
        snapshot
            .raw_registers
            .insert(method::SET_VERTEX_ELEMENT_BASE, 0);
        snapshot
            .raw_registers
            .insert(method::SET_PRIMITIVE_RESTART, 2);
        assert!(matches!(
            prepare_captured_indexed_vertex_inputs(
                &memory,
                &gpu,
                &event(&snapshot),
                &interfaces,
                &resources
            ),
            Err(IndexReadError::InvalidState)
        ));
        snapshot
            .raw_registers
            .insert(method::SET_PRIMITIVE_RESTART, 1);
        assert!(matches!(
            prepare_captured_indexed_vertex_inputs(
                &memory,
                &gpu,
                &event(&snapshot),
                &interfaces,
                &resources
            ),
            Err(IndexReadError::InvalidState)
        ));
        memory.write(0x4004, &[1, 0, 4, 0]).unwrap();
        assert!(matches!(
            prepare_indexed_vertex_inputs(
                &memory,
                &gpu,
                selection,
                &snapshot,
                &interfaces,
                &resources
            ),
            Err(IndexReadError::Vertex(_))
        ));
        assert!(read_indices(&memory, &gpu, state, 2).is_ok());
        state.limit.b -= 1;
        assert!(matches!(
            read_indices(&memory, &gpu, state, 2),
            Err(IndexReadError::LimitExceeded)
        ));
        state.limit.b += 1;
        let foreign = memory.clone();
        assert!(matches!(
            read_indices(&foreign, &gpu, state, 2),
            Err(IndexReadError::Memory(_))
        ));
        gpu.unmap(0x100000).unwrap();
        assert!(matches!(
            read_indices(&memory, &gpu, state, 2),
            Err(IndexReadError::Memory(_))
        ));
    }
}
