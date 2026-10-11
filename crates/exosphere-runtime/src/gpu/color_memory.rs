// SPDX-License-Identifier: GPL-3.0-or-later
//! Color-layer pixel transfers through canonical live GPU mappings.

use exosphere_gpu_ir::{TextureFormat, maxwell_state::DrawStateSnapshot};
use exosphere_horizon::{
    HorizonError,
    memory::{AddressSpace, MemoryPermission},
    nvdrv::address_space::NvGpuAddressSpace,
};

const MAX_COLOR_LAYER_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ColorTransferError {
    #[error("missing or unsupported color-layer descriptor or payload format")]
    InvalidDescriptor,
    #[error("color payload does not match the layer's tightly packed size")]
    InvalidPayload,
    #[error("color-layer conversion exceeds the software allocation budget")]
    Budget,
    #[error(transparent)]
    Memory(#[from] HorizonError),
}

/// Existing writeback callers use the same transfer failures.
pub type ColorWriteError = ColorTransferError;

/// Read the selected live layer as tightly packed exact-format bytes. This is
/// not a cached host image and performs no numeric/channel conversion.
pub fn read_color_layer(
    memory: &AddressSpace,
    gpu: &NvGpuAddressSpace,
    snapshot: &DrawStateSnapshot,
    index: usize,
    format: TextureFormat,
) -> Result<Vec<u8>, ColorTransferError> {
    let target = snapshot
        .color_target(index)
        .ok_or(ColorTransferError::InvalidDescriptor)?;
    if target.canonical_format() != Some(format) {
        return Err(ColorTransferError::InvalidDescriptor);
    }
    let layout = target
        .block_linear_layer_layout()
        .ok_or(ColorTransferError::InvalidDescriptor)?;
    let (address, bytes) = target
        .block_linear_layer_range()
        .ok_or(ColorTransferError::InvalidDescriptor)?;
    if bytes > MAX_COLOR_LAYER_BYTES {
        return Err(ColorTransferError::Budget);
    }
    let size = usize::try_from(bytes).map_err(|_| ColorTransferError::Budget)?;
    gpu.validate_access(memory, address, size, MemoryPermission::READ)?;
    let mut tiled = Vec::new();
    tiled
        .try_reserve_exact(size)
        .map_err(|_| ColorTransferError::Budget)?;
    tiled.resize(size, 0);
    gpu.read(memory, address, &mut tiled)?;
    exosphere_gpu_ir::block_linear::deswizzle_layers(
        &tiled,
        layout.width_bytes,
        layout.height_rows,
        1,
        1,
        layout.block_height_log2,
        0,
    )
    .ok_or(ColorTransferError::InvalidPayload)
}

/// Write completed tightly packed pixels in the exact declared format. This
/// does not convert RGBA8 readback to other formats or signal a guest fence.
pub fn write_color_layer(
    memory: &mut AddressSpace,
    gpu: &NvGpuAddressSpace,
    snapshot: &DrawStateSnapshot,
    index: usize,
    format: TextureFormat,
    pixels: &[u8],
) -> Result<(), ColorWriteError> {
    let target = snapshot
        .color_target(index)
        .ok_or(ColorWriteError::InvalidDescriptor)?;
    if target.canonical_format() != Some(format) {
        return Err(ColorWriteError::InvalidDescriptor);
    }
    let layout = target
        .block_linear_layer_layout()
        .ok_or(ColorWriteError::InvalidDescriptor)?;
    let (address, bytes) = target
        .block_linear_layer_range()
        .ok_or(ColorWriteError::InvalidDescriptor)?;
    let linear_bytes = u64::from(layout.width_bytes) * u64::from(layout.height_rows);
    if pixels.len() as u64 != linear_bytes {
        return Err(ColorWriteError::InvalidPayload);
    }
    if bytes > MAX_COLOR_LAYER_BYTES {
        return Err(ColorWriteError::Budget);
    }
    let size = usize::try_from(bytes).map_err(|_| ColorWriteError::Budget)?;
    // Reading the old padded layer preserves bytes outside logical pixels. Both
    // permissions and every backing lease are checked before any modification.
    gpu.validate_access(memory, address, size, MemoryPermission::READ_WRITE)?;
    let mut tiled = Vec::new();
    tiled
        .try_reserve_exact(size)
        .map_err(|_| ColorWriteError::Budget)?;
    tiled.resize(size, 0);
    gpu.read(memory, address, &mut tiled)?;
    exosphere_gpu_ir::block_linear::swizzle_2d_into(pixels, &mut tiled, layout)
        .ok_or(ColorWriteError::InvalidPayload)?;
    gpu.write(memory, address, &tiled)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use exosphere_horizon::{
        memory::{MemoryKind, PAGE_SIZE},
        nvdrv::NvMapBacking,
    };

    #[test]
    fn writes_mapped_pixels_preserves_padding_and_rejects_stale_leases() {
        let mut memory = AddressSpace::default();
        memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        memory
            .write(0x4000, &vec![0xa5; PAGE_SIZE as usize])
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
        for (field, word) in [0, 0x100000, 17, 3, 0xd5, 0, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            snapshot.raw_registers.insert(
                exosphere_gpu_ir::maxwell_state::method::COLOR_TARGET_BASE + field as u16,
                word,
            );
        }
        let pixels: Vec<u8> = (0..68 * 3).map(|index| (index % 251) as u8).collect();
        assert!(matches!(
            write_color_layer(
                &mut memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::R32Float,
                &pixels
            ),
            Err(ColorWriteError::InvalidDescriptor)
        ));
        assert!(matches!(
            write_color_layer(
                &mut memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
                &pixels[..203]
            ),
            Err(ColorWriteError::InvalidPayload)
        ));
        write_color_layer(
            &mut memory,
            &gpu,
            &snapshot,
            0,
            TextureFormat::Rgba8Unorm,
            &pixels,
        )
        .unwrap();
        assert_eq!(
            read_color_layer(&memory, &gpu, &snapshot, 0, TextureFormat::Rgba8Unorm).unwrap(),
            pixels
        );
        let changed = vec![0x5a; pixels.len()];
        write_color_layer(
            &mut memory,
            &gpu,
            &snapshot,
            0,
            TextureFormat::Rgba8Unorm,
            &changed,
        )
        .unwrap();
        assert_eq!(
            read_color_layer(&memory, &gpu, &snapshot, 0, TextureFormat::Rgba8Unorm).unwrap(),
            changed
        );
        write_color_layer(
            &mut memory,
            &gpu,
            &snapshot,
            0,
            TextureFormat::Rgba8Unorm,
            &pixels,
        )
        .unwrap();
        let mut tiled = vec![0; 1024];
        gpu.read(&memory, 0x100000, &mut tiled).unwrap();
        assert_eq!(
            exosphere_gpu_ir::block_linear::deswizzle_layers(&tiled, 68, 3, 1, 1, 0, 0).unwrap(),
            pixels
        );
        let layout = snapshot
            .color_target(0)
            .unwrap()
            .block_linear_layer_layout()
            .unwrap();
        let mut used = vec![false; 1024];
        for y in 0..3 {
            for x in 0..68 {
                used[layout.offset(x, y, 0).unwrap() as usize] = true;
            }
        }
        assert!(
            tiled
                .iter()
                .zip(used)
                .all(|(byte, used)| used || *byte == 0xa5)
        );
        let mut foreign = memory.clone();
        assert!(matches!(
            write_color_layer(
                &mut foreign,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
                &pixels
            ),
            Err(ColorWriteError::Memory(_))
        ));
        gpu.unmap(0x100000).unwrap();
        assert!(matches!(
            write_color_layer(
                &mut memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
                &pixels
            ),
            Err(ColorWriteError::Memory(_))
        ));
        let readonly = NvMapBacking {
            lease: memory
                .pin_device_memory(0x4000, PAGE_SIZE, MemoryPermission::READ)
                .unwrap(),
            ..backing
        };
        gpu.map_fixed(0x100000, PAGE_SIZE, 0, &readonly).unwrap();
        assert_eq!(
            read_color_layer(&memory, &gpu, &snapshot, 0, TextureFormat::Rgba8Unorm).unwrap(),
            pixels
        );
        assert!(matches!(
            write_color_layer(
                &mut memory,
                &gpu,
                &snapshot,
                0,
                TextureFormat::Rgba8Unorm,
                &changed,
            ),
            Err(ColorWriteError::Memory(_))
        ));
        let mut after = vec![0; 1024];
        gpu.read(&memory, 0x100000, &mut after).unwrap();
        assert_eq!(after, tiled);
    }
}
