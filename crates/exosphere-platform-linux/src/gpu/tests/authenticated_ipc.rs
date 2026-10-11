// SPDX-License-Identifier: GPL-3.0-or-later
//! Adapter admission through actual kernel-owned nvdrv channel identities.

use super::super::*;
use exosphere_horizon::{
    HorizonProfileId, cmif,
    handle::Handle,
    hipc,
    kernel::HorizonKernel,
    memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
    nvdrv::NvGpuChannelIdentity,
};
use exosphere_runtime::gpu::nv_backend::NativeDrawRenderer;

/// Encoded shift cases project one result bit to black/white via LOP and I2F.
/// Keeping this separate avoids mixing count/sign oracles with shader encoding.
struct ShiftFixture {
    kind: exosphere_gpu_ir::shader::WordShiftKind,
    wrap: bool,
    amount: u32,
    value: u32,
    mask: u32,
    source: WordSource,
}

#[derive(Clone, Copy)]
enum WordSource {
    Register,
    Immediate,
    ConstantBuffer,
}

impl ShiftFixture {
    const VARIANTS: usize = 432;

    fn from_variant(variant: usize) -> Self {
        use exosphere_gpu_ir::shader::WordShiftKind as K;
        assert!(variant < Self::VARIANTS);
        Self {
            source: match variant % 3 {
                0 => WordSource::Register,
                1 => WordSource::Immediate,
                _ => WordSource::ConstantBuffer,
            },
            mask: [1, 0x80000000][(variant / 3) % 2],
            value: [0x80000001, 0x7fffffff][(variant / 6) % 2],
            amount: [0, 1, 31, 32, 33, u32::MAX][(variant / 12) % 6],
            wrap: (variant / 72) % 2 != 0,
            kind: [K::Left, K::RightLogical, K::RightArithmetic][variant / 144],
        }
    }

    fn expected_color(&self) -> [u8; 4] {
        use exosphere_gpu_ir::shader::WordShiftKind as K;
        let count = if self.wrap {
            self.amount & 31
        } else {
            self.amount.min(32)
        };
        let word = match self.kind {
            K::Left => self.value.checked_shl(count).unwrap_or(0),
            K::RightLogical => self.value.checked_shr(count).unwrap_or(0),
            K::RightArithmetic => ((self.value as i32) >> count.min(31)) as u32,
        };
        let level = if word & self.mask == 0 { 0 } else { 255 };
        [level, level, level, 255]
    }

    fn instructions(&self) -> [u64; 12] {
        use exosphere_gpu_ir::shader::WordShiftKind as K;
        let suffix = if self.kind == K::Left {
            0x0048000000000000
        } else {
            0x0028000000000000
        };
        let shift = match self.source {
            WordSource::Immediate => {
                0x3800000000000000
                    | suffix
                    | (u64::from(self.amount & 0x7ffff) << 20)
                    | (u64::from((self.amount >> 19) & 1) << 56)
            }
            WordSource::Register => 0x5c00000000000000 | suffix | (9 << 20),
            // Fragment c[1][0] is independently bound below, away from depth.
            WordSource::ConstantBuffer => 0x4c00000000000000 | suffix | (1 << 34),
        } | (8 << 8)
            | (7 << 16)
            | (u64::from(self.wrap) << 39)
            | (u64::from(self.kind == K::RightArithmetic) << 48);
        word_projection_instructions(self.value, self.amount, shift, self.mask)
    }
}

/// Keep the encoded BFE field and its independent bitwise oracle together.
/// Register/constant controls contain ignored high bits; the short immediate
/// truncates/sign-extends those bits without changing the packed field bytes.
struct ExtractFixture {
    source: WordSource,
    signed: bool,
    offset: u32,
    width: u32,
    value: u32,
    mask: u32,
}

impl ExtractFixture {
    const VARIANTS: usize = 3 * 2 * 7 * 7 * 2 * 2;

    fn from_variant(variant: usize) -> Self {
        assert!(variant < Self::VARIANTS);
        Self {
            source: [
                WordSource::Register,
                WordSource::Immediate,
                WordSource::ConstantBuffer,
            ][variant % 3],
            mask: [1, 0x80000000][(variant / 3) % 2],
            value: [0x80000001, 0x7fffffff][(variant / 6) % 2],
            offset: [0, 1, 7, 31, 32, 33, 255][(variant / 12) % 7],
            width: [0, 1, 8, 31, 32, 33, 255][(variant / 84) % 7],
            signed: variant / 588 != 0,
        }
    }

    fn control(&self) -> u32 {
        0xffff0000 | (self.width << 8) | self.offset
    }

    fn expected_color(&self) -> [u8; 4] {
        let sign = self.signed
            && self.width != 0
            && (self.value >> (self.offset + self.width - 1).min(31)) & 1 != 0;
        let mut word = 0u32;
        for bit in 0..32 {
            let set = if bit < self.width && self.offset + bit < 32 {
                self.value & (1 << (self.offset + bit)) != 0
            } else {
                sign
            };
            word |= u32::from(set) << bit;
        }
        let level = if word & self.mask == 0 { 0 } else { 255 };
        [level, level, level, 255]
    }

    fn instructions(&self) -> [u64; 12] {
        let control = self.control();
        let instruction = match self.source {
            WordSource::Register => 0x5c00000000000000 | (9 << 20),
            WordSource::Immediate => {
                0x3800000000000000
                    | (u64::from(control & 0x7ffff) << 20)
                    | (u64::from((control >> 19) & 1) << 56)
            }
            WordSource::ConstantBuffer => 0x4c00000000000000 | (1 << 34),
        } | (8 << 8)
            | (7 << 16)
            | (u64::from(self.signed) << 48);
        word_projection_instructions(self.value, control, instruction, self.mask)
    }
}

struct UnaryWordFixture {
    source: WordSource,
    value: u32,
    invert: bool,
    mask: u32,
}

impl UnaryWordFixture {
    const VARIANTS: usize = 3 * 2 * 8 * 6;

    fn from_variant(variant: usize) -> Self {
        assert!(variant < Self::VARIANTS);
        Self {
            source: [
                WordSource::Register,
                WordSource::Immediate,
                WordSource::ConstantBuffer,
            ][variant % 3],
            invert: (variant / 3) % 2 != 0,
            value: [
                0,
                1,
                0x7ffff,
                0xfff80000,
                u32::MAX,
                0x55555,
                0xfffaaaaa,
                0x80000000,
            ][(variant / 6) % 8],
            mask: 1 << (variant / 48),
        }
    }

    fn source_word(&self) -> u32 {
        // Immediate operations read the sign-extended 20-bit operand, not the
        // original full word used by the register/constant forms.
        let source = if matches!(self.source, WordSource::Immediate) {
            ((self.value << 12) as i32 >> 12) as u32
        } else {
            self.value
        };
        if self.invert { !source } else { source }
    }

    fn count_color(&self) -> [u8; 4] {
        let source = self.source_word();
        let count = (0..32).filter(|bit| source & (1 << bit) != 0).count() as u32;
        let level = if count & self.mask == 0 { 0 } else { 255 };
        [level, level, level, 255]
    }

    fn operation(&self, suffix: u64) -> u64 {
        (match self.source {
            WordSource::Register => 0x5c00000000000000 | (8 << 20),
            WordSource::Immediate => {
                0x3800000000000000
                    | (u64::from(self.value & 0x7ffff) << 20)
                    | (u64::from((self.value >> 19) & 1) << 56)
            }
            WordSource::ConstantBuffer => 0x4c00000000000000 | (1 << 34),
        }) | suffix
            | (7 << 16)
            | (u64::from(self.invert) << 40)
    }

    fn count_instructions(&self) -> [u64; 12] {
        let instruction = self.operation(0x0008000000000000);
        word_projection_instructions(self.value, self.value, instruction, self.mask)
    }
}

struct LeadingFixture {
    input: UnaryWordFixture,
    signed: bool,
    shift_amount: bool,
}

impl LeadingFixture {
    const VARIANTS: usize = UnaryWordFixture::VARIANTS * 4;

    fn from_variant(variant: usize) -> Self {
        assert!(variant < Self::VARIANTS);
        let mut input = UnaryWordFixture::from_variant(variant % UnaryWordFixture::VARIANTS);
        // Indices/shift amounts fit five bits; the high projection checks the
        // not-found sentinel instead of the population-count result's bit 5.
        if input.mask == 32 {
            input.mask = 0x80000000;
        }
        Self {
            input,
            signed: (variant / UnaryWordFixture::VARIANTS) % 2 != 0,
            shift_amount: variant / (UnaryWordFixture::VARIANTS * 2) != 0,
        }
    }

    fn expected_color(&self) -> [u8; 4] {
        let word = self.input.source_word();
        let negative = self.signed && word & 0x80000000 != 0;
        let found = (0..32)
            .rev()
            .find(|bit| (word & (1 << bit) != 0) != negative);
        let result = found.map_or(
            u32::MAX,
            |bit| if self.shift_amount { 31 - bit } else { bit },
        );
        let level = if result & self.input.mask == 0 {
            0
        } else {
            255
        };
        [level, level, level, 255]
    }

    fn instructions(&self) -> [u64; 12] {
        let instruction = self.input.operation(0x0030000000000000)
            | (u64::from(self.signed) << 48)
            | (u64::from(self.shift_amount) << 41);
        word_projection_instructions(
            self.input.value,
            self.input.value,
            instruction,
            self.input.mask,
        )
    }
}

/// Project an integer result into RGB without reinterpreting it as a float.
fn word_projection_instructions(
    value: u32,
    control: u32,
    instruction: u64,
    mask: u32,
) -> [u64; 12] {
    let mov = |dst: u64, word: u32| 0x010000000007f000u64 | (u64::from(word) << 20) | dst;
    let copy = |dst: u64| 0x5c98000000000000u64 | (15 << 39) | (7 << 16) | dst;
    [
        0,
        mov(8, value),
        mov(9, control),
        instruction,
        0,
        0x0400000000000000 | (u64::from(mask) << 20) | (7 << 16),
        0x5cb8000000000000 | (2 << 8) | (2 << 10) | (7 << 16),
        copy(1),
        0,
        copy(2),
        mov(3, 1.0f32.to_bits()),
        0xe30000000007000f,
    ]
}

fn srgb_texture_pixels(refresh: bool) -> [[u8; 4]; 4] {
    [
        [64, 128, 192, 128],
        [128, 192, 64, 128],
        [192, 64, 128, 128],
        if refresh {
            [32, 96, 160, 192]
        } else {
            [192, 128, 64, 128]
        },
    ]
}

/// A partial 2x2 BC1 view occupies selector positions 0, 1, 4, 5.
/// Red/green endpoints make the expected center sample independent of a decoder.
fn bc1_texture_block(refresh: bool) -> Vec<u8> {
    let mut block = vec![0; 8];
    block[..2].copy_from_slice(&0xf800u16.to_le_bytes());
    block[2..4].copy_from_slice(&0x07e0u16.to_le_bytes());
    let selectors = (1u32 << 2) | (1 << 8) | (u32::from(refresh) << 10);
    block[4..].copy_from_slice(&selectors.to_le_bytes());
    block
}

fn compressed_texture_block(format: usize, refresh: bool) -> Vec<u8> {
    let color = bc1_texture_block(refresh);
    match format {
        5 => color,
        6 => {
            // BC2 stores one four-bit alpha per texel. The visible values
            // [0,4,8,12] average to 102/255; refresh changes 12 to 8 (85/255).
            let alpha = (4u64 << 4) | (8 << 16) | ((if refresh { 8 } else { 12 }) << 20);
            [alpha.to_le_bytes().as_slice(), &color].concat()
        }
        7 => {
            // BC3 endpoint selectors avoid implementation-dependent rounding
            // of interpolated alpha codes: [0,128,0,128] then [0,128,0,0].
            let mut alpha = [0u8; 8];
            alpha[1] = 128;
            let selectors = (1u64 << 3) | (u64::from(!refresh) << 15);
            alpha[2..].copy_from_slice(&selectors.to_le_bytes()[..6]);
            [alpha.as_slice(), &color].concat()
        }
        _ => unreachable!("unknown compressed fixture format"),
    }
}

fn srgb_texture_reference(refresh: bool) -> [u8; 4] {
    let pixels = srgb_texture_pixels(refresh);
    let decode = |byte| {
        let encoded = f64::from(byte) / 255.0;
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    // Convert each source texel before bilinear filtering, then apply the TIC
    // [B,R,G,A] swizzle. Alpha is filtered as UNORM, never decoded as sRGB.
    [2, 0, 1, 3].map(|channel| {
        let average = pixels
            .iter()
            .map(|pixel| {
                if channel == 3 {
                    f64::from(pixel[channel]) / 255.0
                } else {
                    decode(pixel[channel])
                }
            })
            .sum::<f64>()
            / 4.0;
        (average * 255.0).round() as u8
    })
}

struct Driver {
    kernel: HorizonKernel,
    process: u64,
    thread: u64,
    session: Handle,
    tls: u64,
}

impl Driver {
    fn new() -> Self {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let process = kernel.create_process(1, "Linux adapter IPC test");
        let (thread, _) = kernel.create_thread(process, 0x1000, 0xb000, 44).unwrap();
        for (address, size) in [(0x4000, PAGE_SIZE), (0x8000, 3 * PAGE_SIZE)] {
            kernel
                .process_mut(process)
                .unwrap()
                .memory
                .map(
                    address,
                    size,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
        }
        let transfer = kernel
            .create_transfer_memory(process, 0x4000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let session = kernel.connect_service(process, "nvdrv").unwrap();
        let tls = kernel.thread(thread).unwrap().context.tpidrro_el0;
        let mut driver = Self {
            kernel,
            process,
            thread,
            session,
            tls,
        };
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&0x80000009u32.to_le_bytes());
        // Initialization copies the process pseudo-handle and transfer memory.
        request[8..12].copy_from_slice(&4u32.to_le_bytes());
        request[12..16].copy_from_slice(&0xffff8001u32.to_le_bytes());
        request[16..20].copy_from_slice(&transfer.0.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&3u32.to_le_bytes());
        request[48..52].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        assert_eq!(&driver.invoke(request)[..4], &0u32.to_le_bytes());
        driver.kernel.close_handle(process, transfer).unwrap();
        driver
    }

    fn invoke(&mut self, request: [u8; 0x100]) -> Vec<u8> {
        self.kernel
            .process_mut(self.process)
            .unwrap()
            .memory
            .write(self.tls, &request)
            .unwrap();
        self.kernel
            .send_guest_sync_request(self.process, self.thread, self.session)
            .unwrap();
        let mut bytes = [0; 0x100];
        self.kernel
            .process(self.process)
            .unwrap()
            .memory
            .read(self.tls, &mut bytes)
            .unwrap();
        let message = hipc::parse(&bytes).unwrap();
        let response = cmif::parse_response(message.raw_data).unwrap();
        assert_eq!(response.header.result, 0);
        response.payload.to_vec()
    }

    fn open(&mut self, path: &[u8]) -> u32 {
        self.kernel
            .process_mut(self.process)
            .unwrap()
            .memory
            .write(0x8000, path)
            .unwrap();
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&(4u32 | (1 << 20)).to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[8..12].copy_from_slice(&(path.len() as u32).to_le_bytes());
        request[12..16].copy_from_slice(&0x8000u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        let reply = self.invoke(request);
        assert_eq!(&reply[4..8], &0u32.to_le_bytes());
        u32::from_le_bytes(reply[..4].try_into().unwrap())
    }

    fn ioctl(&mut self, descriptor: u32, code: u32, data: &[u8]) {
        self.ioctl_buffer(descriptor, code, data, false);
    }

    fn ioctl_buffer(&mut self, descriptor: u32, code: u32, data: &[u8], inout: bool) -> Vec<u8> {
        self.kernel
            .process_mut(self.process)
            .unwrap()
            .memory
            .write(0x9000, data)
            .unwrap();
        let mut request = [0; 0x100];
        let receive = if inout { 1 << 24 } else { 0 };
        request[..4].copy_from_slice(&(4u32 | (1 << 20) | receive).to_le_bytes());
        request[4..8].copy_from_slice(&10u32.to_le_bytes());
        request[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
        request[12..16].copy_from_slice(&0x9000u32.to_le_bytes());
        if inout {
            request[20..24].copy_from_slice(&(data.len() as u32).to_le_bytes());
            request[24..28].copy_from_slice(&0xa000u32.to_le_bytes());
        }
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&1u32.to_le_bytes());
        request[48..52].copy_from_slice(&descriptor.to_le_bytes());
        request[52..56].copy_from_slice(&code.to_le_bytes());
        assert_eq!(&self.invoke(request)[..4], &0u32.to_le_bytes());
        let mut output = vec![0; if inout { data.len() } else { 0 }];
        if inout {
            self.kernel
                .process(self.process)
                .unwrap()
                .memory
                .read(0xa000, &mut output)
                .unwrap();
        }
        output
    }

    fn channel(&mut self) -> u32 {
        self.devices().2
    }

    fn devices(&mut self) -> (u32, u32, u32) {
        let map = self.open(b"/dev/nvmap");
        let space = self.open(b"/dev/nvhost-as-gpu");
        let channel = self.open(b"/dev/nvhost-gpu");
        self.ioctl(space, 0x40284109, &[0; 40]);
        self.ioctl(space, 0x40044101, &channel.to_le_bytes());
        self.ioctl(channel, 0x40044801, &map.to_le_bytes());
        (map, space, channel)
    }

    fn close(&mut self, descriptor: u32) {
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&9u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&2u32.to_le_bytes());
        request[32..36].copy_from_slice(&descriptor.to_le_bytes());
        assert_eq!(&self.invoke(request)[..4], &0u32.to_le_bytes());
    }
}

#[derive(Debug)]
struct CleanupProbe(std::sync::Arc<std::sync::Mutex<Vec<NvGpuChannelIdentity>>>);

#[derive(Debug)]
struct RenderingPrograms {
    inner: DirectDrawProgramProvider,
    closed: std::sync::Arc<std::sync::Mutex<Vec<NvGpuChannelIdentity>>>,
}

impl DrawProgramProvider for RenderingPrograms {
    fn resolve(
        &mut self,
        identity: &NvGpuChannelIdentity,
        memory: &AddressSpace,
        gpu: &NvGpuAddressSpace,
        state: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError> {
        self.inner.resolve(identity, memory, gpu, state)
    }

    fn close_channel(&mut self, identity: &NvGpuChannelIdentity) {
        self.closed.lock().unwrap().push(identity.clone());
    }
}

#[test]
#[ignore = "requires an explicitly selected Vulkan 1.3 device"]
fn native_authenticated_linux_adapter_renders_live_payload_and_closes_used_channel() {
    use exosphere_runtime::gpu::{
        nv_backend::NativeGpuBackend, shader_memory::DirectDrawShaderGroups,
    };

    let mut driver = Driver::new();
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .map(
            0xb000,
            2 * PAGE_SIZE,
            MemoryPermission::READ_WRITE,
            MemoryKind::Heap,
        )
        .unwrap();
    let (map, space, channel) = driver.devices();
    let created = driver.ioctl_buffer(
        map,
        0xc0080101,
        &[(2 * PAGE_SIZE as u32).to_le_bytes(), 0u32.to_le_bytes()].concat(),
        true,
    );
    let handle = u32::from_le_bytes(created[4..8].try_into().unwrap());
    let mut allocation = [0; 32];
    allocation[..4].copy_from_slice(&handle.to_le_bytes());
    allocation[8..12].copy_from_slice(&1u32.to_le_bytes());
    allocation[24..32].copy_from_slice(&0xb000u64.to_le_bytes());
    driver.ioctl_buffer(map, 0xc0200104, &allocation, true);
    for (address, pages, offset) in [(0x100000u64, 2u32, 0u64), (0x200000, 1, PAGE_SIZE)] {
        let mut reservation = [0; 24];
        reservation[..4].copy_from_slice(&pages.to_le_bytes());
        reservation[4..8].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        reservation[8..12].copy_from_slice(&1u32.to_le_bytes());
        reservation[16..24].copy_from_slice(&address.to_le_bytes());
        driver.ioctl_buffer(space, 0xc0184102, &reservation, true);
        let mut mapping = [0; 40];
        mapping[..4].copy_from_slice(&1u32.to_le_bytes());
        mapping[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        mapping[8..12].copy_from_slice(&handle.to_le_bytes());
        mapping[12..16].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        mapping[16..24].copy_from_slice(&offset.to_le_bytes());
        mapping[24..32].copy_from_slice(&(u64::from(pages) * PAGE_SIZE).to_le_bytes());
        mapping[32..40].copy_from_slice(&address.to_le_bytes());
        driver.ioctl_buffer(space, 0xc0284106, &mapping, true);
    }
    let closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = RenderingPrograms {
        inner: DirectDrawProgramProvider::new(
            4096,
            DirectDrawShaderGroups {
                vertex_group: 0,
                fragment_group: 4,
                vertex_set: 0,
                fragment_set: 1,
            },
        )
        .unwrap(),
        closed: closed.clone(),
    };
    let mut renderer = LinuxDrawRenderer::new(provider, 1).unwrap();
    let mut cases = Vec::new();
    let owner = driver
        .kernel
        .with_gpu_channel(
            driver.process,
            driver.session,
            channel,
            |owner, memory, gpu| {
                super::verify_vertex_uniform_fixture(
                    memory,
                    gpu,
                    0xb000,
                    |memory, gpu, event, _| {
                        let mut vertices = vec![0; 48];
                        let mut depth = [0; 4];
                        memory.read(0xc080, &mut vertices).unwrap();
                        memory.read(0xc000, &mut depth).unwrap();
                        renderer.draw(&owner, memory, gpu, event).unwrap();
                        assert_eq!(renderer.channels.len(), 1);
                        let (identity, native) = &mut renderer.channels[0];
                        assert_eq!(identity, &owner);
                        assert_eq!(native.images.len(), 1);
                        let pixels = native.device.readback_rgba8(native.images[0].3).unwrap();
                        cases.push((event.clone(), vertices, depth, pixels.clone()));
                        pixels
                    },
                );
                owner
            },
        )
        .unwrap();
    // Replay fixture inputs through actual IPC admission and kernel polling.
    // These small test payloads are not driver memory or mapping snapshots.
    driver
        .kernel
        .install_gpu_ioctl_backend(Box::new(
            NativeGpuBackend::with_renderer(1, Box::new(renderer)).unwrap(),
        ))
        .unwrap();
    let mut queue = [0; 32];
    queue[..4].copy_from_slice(&2u32.to_le_bytes());
    queue[4..8].copy_from_slice(&1u32.to_le_bytes());
    driver.ioctl_buffer(channel, 0xc020481a, &queue, true);
    let mut object = [0; 16];
    object[..4].copy_from_slice(&0xb197u32.to_le_bytes());
    driver.ioctl_buffer(channel, 0xc0104809, &object, true);
    let mut last_submission = [0; 32];
    let mut saved_depth_instruction = [0; 8];
    let mut saved_w_instruction = [0; 8];
    let mut saved_fragment_body = [0; 96];
    driver
        .kernel
        .process(driver.process)
        .unwrap()
        .memory
        .read(0xc160, &mut saved_depth_instruction)
        .unwrap();
    let memory = &driver.kernel.process(driver.process).unwrap().memory;
    memory.read(0xc168, &mut saved_w_instruction).unwrap();
    memory.read(0xc350, &mut saved_fragment_body).unwrap();
    let mut admission_front = exosphere_gpu_ir::frontend::MaxwellFrontEnd::new();
    const MIP_START: usize = 185 + ShiftFixture::VARIANTS;
    const EXTRACT_START: usize = MIP_START + 16;
    const POP_COUNT_START: usize = EXTRACT_START + ExtractFixture::VARIANTS;
    const LEADING_START: usize = POP_COUNT_START + UnaryWordFixture::VARIANTS;
    // Ordered modes: ordinary, indexed, restart, instance sequence, VertexID
    // bases, uniform/immediate I2F, scalar varyings, then live textured draws.
    for ((event, vertices, depth, mut expected), mode) in cases.into_iter().flat_map(|case| {
        (0..LEADING_START + LeadingFixture::VARIANTS).map(move |mode| (case.clone(), mode))
    }) {
        use exosphere_gpu_ir::maxwell_state::method;
        let indexed = mode != 0;
        let restart = mode == 2;
        let instanced = (3..=8).contains(&mode);
        let vertex_id = (9..=11).contains(&mode);
        let conversion = (12..60).contains(&mode).then(|| {
            let variant = (mode - 12) % 24;
            (variant >= 12, (variant / 3) % 4, variant % 3, mode >= 36)
        });
        let varying = (60..63)
            .contains(&mode)
            .then(|| [0x80u64, 0x88, 0x94][mode - 60]);
        let arithmetic = (127..149).contains(&mode).then(|| {
            let variant = mode - 127;
            match variant {
                0..=8 => (variant >= 3, variant >= 6, variant % 3, 0),
                9..=11 => (false, false, variant - 9, 1),
                12..=13 => (false, false, variant - 12, 2),
                14..=15 => (false, false, variant - 14, 3),
                16..=21 => (variant >= 19, false, 0, 0),
                _ => unreachable!("unknown integer modifier fixture"),
            }
        });
        let zero_sources = if (143..149).contains(&mode) {
            [1, 2, 3][(mode - 143) % 3]
        } else {
            0
        };
        let logic = (149..185).contains(&mode).then(|| {
            let variant = mode - 149;
            (variant / 12, (variant / 3) % 4, variant % 3)
        });
        let shift = (185..MIP_START)
            .contains(&mode)
            .then(|| ShiftFixture::from_variant(mode - 185));
        let extract = (EXTRACT_START..POP_COUNT_START)
            .contains(&mode)
            .then(|| ExtractFixture::from_variant(mode - EXTRACT_START));
        let pop_count = (POP_COUNT_START..LEADING_START)
            .contains(&mode)
            .then(|| UnaryWordFixture::from_variant(mode - POP_COUNT_START));
        let leading =
            (mode >= LEADING_START).then(|| LeadingFixture::from_variant(mode - LEADING_START));
        // Two formats, nearest/half-blended mip selection, implicit/zero LOD,
        // and live refresh. Base shader coordinates remain the texture center.
        let mip_texture = (MIP_START..EXTRACT_START).contains(&mode).then(|| {
            let variant = mode - MIP_START;
            (
                variant % 8 >= 4,
                variant % 4 >= 2,
                variant % 2 != 0,
                if variant >= 8 { 5 } else { 0 },
            )
        });
        let word_color = shift
            .as_ref()
            .map(ShiftFixture::expected_color)
            .or_else(|| extract.as_ref().map(ExtractFixture::expected_color))
            .or_else(|| pop_count.as_ref().map(UnaryWordFixture::count_color))
            .or_else(|| leading.as_ref().map(LeadingFixture::expected_color));
        if let Some(color) = word_color {
            for pixel in expected.chunks_exact_mut(4) {
                if pixel != [17, 43, 91, 255] {
                    pixel.copy_from_slice(&color);
                }
            }
        }
        if let Some((function, _, _)) = logic {
            let value = if function == 1 { 131 } else { 128 };
            for pixel in expected.chunks_exact_mut(4) {
                if pixel != [17, 43, 91, 255] {
                    pixel.copy_from_slice(&[value, value, value, 255]);
                }
            }
        }
        let texture = (63..127)
            .contains(&mode)
            .then(|| {
                let variant = mode - 63;
                (
                    if variant % 8 < 4 { 2 } else { 3 },
                    variant % 4 >= 2,
                    variant % 2 != 0,
                    variant / 8,
                )
            })
            .or_else(|| mip_texture.map(|(_, zero, refresh, format)| (3, zero, refresh, format)));
        if let Some((multiply, _, _, _)) = arithmetic {
            let value = if zero_sources != 0 && (multiply || zero_sources == 3) {
                0
            } else if multiply {
                64
            } else {
                128
            };
            for pixel in expected.chunks_exact_mut(4) {
                if pixel != [17, 43, 91, 255] {
                    pixel.copy_from_slice(&[value, value, value, 255]);
                }
            }
        }
        if let Some((_, _, refresh, format)) = texture {
            for pixel in expected.chunks_exact_mut(4) {
                if pixel != [17, 43, 91, 255] {
                    let value = if refresh { 64 } else { 128 };
                    pixel.copy_from_slice(&match format {
                        0 => [value, value, value, 255],
                        1 => [value, 0, 0, 255],
                        2 => [value, value, 0, 255],
                        3 => [255, 0, value, 255],
                        4 => srgb_texture_reference(refresh),
                        5..=7 => {
                            let alpha = match (format, refresh) {
                                (6, false) => 102,
                                (6, true) => 85,
                                (7, false) => 64,
                                (7, true) => 32,
                                _ => 255,
                            };
                            if refresh {
                                [0, 64, 191, alpha]
                            } else {
                                [0, 128, 128, alpha]
                            }
                        }
                        _ => unreachable!("unknown texture fixture format"),
                    });
                }
            }
        }
        if let Some((blend, _, refresh, format)) = mip_texture {
            let mip_color: [u8; 4] = match (format, refresh) {
                (5, false) => [0, 255, 0, 255],
                (5, true) => [0, 0, 255, 255],
                (_, false) => [91, 17, 43, 255],
                (_, true) => [113, 71, 89, 255],
            };
            for pixel in expected.chunks_exact_mut(4) {
                if pixel != [17, 43, 91, 255] {
                    for (base, mip) in pixel.iter_mut().zip(mip_color) {
                        *base = if blend {
                            (u16::from(*base) + u16::from(mip)).div_ceil(2) as u8
                        } else {
                            mip
                        };
                    }
                }
            }
        }
        if varying.is_some() {
            for (index, pixel) in expected.chunks_exact_mut(4).enumerate() {
                if pixel != [17, 43, 91, 255] {
                    let x = (index % 32) as f32 + 0.5;
                    pixel[0] = ((x / 16.0 - 1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        let instance_flags = match mode {
            4 | 5 | 7 => method::BEGIN_INSTANCE_NEXT,
            6 => method::BEGIN_INSTANCE_CONTINUE,
            _ => 0,
        };
        if (5..=7).contains(&mode)
            || mode == 10
            || conversion.is_some_and(|(_, _, phase, _)| phase == 1)
        {
            expected = [17, 43, 91, 255].repeat(32 * 32);
        }
        let exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
            first,
            count,
            state,
        } = event
        else {
            unreachable!("fixture must contain a vertex-array draw");
        };
        driver
            .kernel
            .with_gpu_channel(driver.process, driver.session, channel, |_, memory, gpu| {
                memory.write(0xc080, &vertices).unwrap();
                memory.write(0xc000, &depth).unwrap();
                let instruction =
                    if let Some((signed, rounding, phase, immediate_source)) = conversion {
                        // Keep the original fixture depth in phases 0/2, but supply
                        // actual integer words rather than its F32 representation.
                        let original_depth = f32::from_le_bytes(depth);
                        assert!(matches!(original_depth, 0.0 | 2.0));
                        let word = if phase == 1 {
                            if signed { u32::MAX } else { 2 }
                        } else {
                            original_depth as u32
                        };
                        let uniform_word = if immediate_source { u32::MAX } else { word };
                        memory.write(0xc000, &uniform_word.to_le_bytes()).unwrap();
                        let source_encoding = if immediate_source {
                            0x38b8000000000000u64
                                | (u64::from(word & 0x7ffff) << 20)
                                | (u64::from((word >> 19) & 1) << 56)
                        } else {
                            0x4cb8000000000000u64
                        };
                        (source_encoding
                            | (2 << 8)
                            | (2 << 10)
                            | (u64::from(signed) << 13)
                            | (7 << 16)
                            | 9
                            | ((rounding as u64) << 39))
                            .to_le_bytes()
                    } else if instanced || vertex_id {
                        (0xefd8000000000000u64
                            | (255 << 39)
                            | (7 << 16)
                            | (255 << 8)
                            | 9
                            | ((if vertex_id { 0x2fc } else { 0x90 }) << 20))
                            .to_le_bytes()
                    } else {
                        saved_depth_instruction
                    };
                memory.write(0xc160, &instruction).unwrap();
                memory.write(0xc168, &saved_w_instruction).unwrap();
                memory.write(0xc350, &saved_fragment_body).unwrap();
                if let Some(attribute) = varying {
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
                    memory.write(0xc168, &store.to_le_bytes()).unwrap();
                    memory.write(0xc358, &interpolate.to_le_bytes()).unwrap();
                }
                if let Some((multiply, signed, form, negation)) = arithmetic {
                    let left = if zero_sources != 0 {
                        0x3f000000u32
                    } else if multiply {
                        0x9f400000u32
                    } else if negation & 1 != 0 {
                        0xc1000001
                    } else {
                        0x3f000001
                    };
                    let right = if zero_sources != 0 {
                        0x3f000000u32
                    } else if multiply {
                        2u32
                    } else if matches!(negation, 1 | 2) {
                        1
                    } else {
                        u32::MAX
                    };
                    let mov =
                        |dst: u64, word: u32| 0x010000000007f000u64 | (u64::from(word) << 20) | dst;
                    memory.write(0xc358, &mov(8, left).to_le_bytes()).unwrap();
                    memory.write(0xc360, &mov(9, right).to_le_bytes()).unwrap();
                    let operation = match (multiply, form) {
                        (false, 0) => 0x5c10000000000000u64 | (9 << 20),
                        (true, 0) => 0x5c38000000000000 | (9 << 20),
                        (false, 1) => {
                            0x3810000000000000
                                | (u64::from(right & 0x7ffff) << 20)
                                | (u64::from((right >> 19) & 1) << 56)
                        }
                        (true, 1) => 0x3838000000000000 | (2 << 20),
                        (false, 2) => 0x1c00000000000000 | (u64::from(right) << 20),
                        (true, 2) => 0x1f00000000000000 | (2 << 20),
                        _ => unreachable!("unknown integer source form"),
                    } | (8 << 8)
                        | (7 << 16)
                        | if signed {
                            3 << if form == 2 { 54 } else { 40 }
                        } else {
                            0
                        };
                    let operation = operation
                        | if negation & 1 != 0 {
                            1 << if form == 2 { 56 } else { 49 }
                        } else {
                            0
                        }
                        | if negation & 2 != 0 { 1 << 48 } else { 0 };
                    let operation = if zero_sources & 1 != 0 {
                        (operation & !(255 << 8)) | (255 << 8)
                    } else {
                        operation
                    };
                    let operation = if zero_sources & 2 != 0 {
                        (operation & !(255 << 20)) | (255 << 20)
                    } else {
                        operation
                    };
                    memory.write(0xc368, &operation.to_le_bytes()).unwrap();
                    let copy = |dst: u64| 0x5c98000000000000u64 | (15 << 39) | (7 << 16) | dst;
                    memory.write(0xc378, &copy(1).to_le_bytes()).unwrap();
                    memory
                        .write(0xc380, &mov(3, 1.0f32.to_bits()).to_le_bytes())
                        .unwrap();
                    memory.write(0xc388, &copy(2).to_le_bytes()).unwrap();
                    // The longer fragment body needs its next schedule word
                    // before EXIT; the saved body restores both after each mode.
                    memory.write(0xc390, &0u64.to_le_bytes()).unwrap();
                    memory
                        .write(0xc398, &0xe30000000007000fu64.to_le_bytes())
                        .unwrap();
                }
                if let Some((function, invert, form)) = logic {
                    // Effective operands produce independent known float words:
                    // AND/XOR => 0x3f000000, OR => 0x3f040000.
                    let (left, right): (u32, u32) = match function {
                        0 => (0x3f0000ff, 0xffffff00),
                        1 => (0x3f000000, 0x40000),
                        2 => (0x3f040000, 0x40000),
                        _ => unreachable!("unknown logic fixture"),
                    };
                    let left = if invert & 1 != 0 { !left } else { left };
                    let right = if invert & 2 != 0 { !right } else { right };
                    let mov =
                        |dst: u64, word: u32| 0x010000000007f000u64 | (u64::from(word) << 20) | dst;
                    memory.write(0xc358, &mov(8, left).to_le_bytes()).unwrap();
                    memory.write(0xc360, &mov(9, right).to_le_bytes()).unwrap();
                    let operation = match form {
                        0 => 0x5c40000000000000u64 | (7 << 48) | (9 << 20),
                        1 => {
                            0x3840000000000000
                                | (7 << 48)
                                | (u64::from(right & 0x7ffff) << 20)
                                | (u64::from((right >> 19) & 1) << 56)
                        }
                        2 => 0x0400000000000000 | (u64::from(right) << 20),
                        _ => unreachable!("unknown logic source form"),
                    } | (8 << 8)
                        | (7 << 16)
                        | ((function as u64) << if form == 2 { 53 } else { 41 })
                        | ((invert as u64) << if form == 2 { 55 } else { 39 });
                    memory.write(0xc368, &operation.to_le_bytes()).unwrap();
                    let copy = |dst: u64| 0x5c98000000000000u64 | (15 << 39) | (7 << 16) | dst;
                    memory.write(0xc378, &copy(1).to_le_bytes()).unwrap();
                    memory
                        .write(0xc380, &mov(3, 1.0f32.to_bits()).to_le_bytes())
                        .unwrap();
                    memory.write(0xc388, &copy(2).to_le_bytes()).unwrap();
                    memory.write(0xc390, &0u64.to_le_bytes()).unwrap();
                    memory
                        .write(0xc398, &0xe30000000007000fu64.to_le_bytes())
                        .unwrap();
                }
                if let Some((control, instructions)) = shift
                    .as_ref()
                    .map(|fixture| (fixture.amount, fixture.instructions()))
                    .or_else(|| {
                        extract
                            .as_ref()
                            .map(|fixture| (fixture.control(), fixture.instructions()))
                    })
                    .or_else(|| {
                        pop_count
                            .as_ref()
                            .map(|fixture| (fixture.value, fixture.count_instructions()))
                    })
                    .or_else(|| {
                        leading
                            .as_ref()
                            .map(|fixture| (fixture.input.value, fixture.instructions()))
                    })
                {
                    // Separate, 256-byte-aligned storage also satisfies the
                    // native device's uniform descriptor offset alignment.
                    memory.write(0xcd00, &[0; 16]).unwrap();
                    memory.write(0xcd00, &control.to_le_bytes()).unwrap();
                    let bytes: Vec<_> = instructions
                        .into_iter()
                        .flat_map(u64::to_le_bytes)
                        .collect();
                    memory.write(0xc350, &bytes).unwrap();
                }
                if let Some((header, level_zero, refresh, format)) = texture {
                    let mov = |dst: u64| {
                        0x010000000007f000u64 | (u64::from(0.5f32.to_bits()) << 20) | dst
                    };
                    let tex = 0xc038000000000000u64
                        | 255 << 20
                        | 1 << 29
                        | 7 << 16
                        | 8 << 8
                        | 15 << 31
                        | u64::from(level_zero) << 55;
                    memory.write(0xc358, &mov(8).to_le_bytes()).unwrap();
                    memory.write(0xc360, &mov(9).to_le_bytes()).unwrap();
                    memory.write(0xc368, &tex.to_le_bytes()).unwrap();
                    // The original fixture copies R2 into alpha here. Preserve
                    // TEX's alpha instead, without changing its extent/EXIT.
                    let keep_alpha = 0x5c98000000000000u64 | 15 << 39 | 3 << 20 | 7 << 16 | 3;
                    memory.write(0xc378, &keep_alpha.to_le_bytes()).unwrap();
                    memory.write(0xc440, &[0; 4]).unwrap(); // TIC 0, TSC 0.
                    let (storage_format, components, swizzle): (u32, usize, [u32; 4]) = match format
                    {
                        0 => (0x08, 4, [4, 2, 3, 5]),
                        1 => (0x1d, 1, [2, 0, 0, 7]),
                        2 => (0x18, 2, [3, 2, 0, 7]),
                        3 => (0x08, 4, [7, 0, 4, 7]),
                        4 => (0x08, 4, [4, 2, 3, 5]),
                        5 => (0x24, 4, [4, 2, 3, 5]),
                        6 => (0x25, 4, [4, 2, 3, 5]),
                        7 => (0x26, 4, [4, 2, 3, 5]),
                        _ => unreachable!("unknown texture fixture format"),
                    };
                    let tic: [u32; 8] = [
                        storage_format
                            | 2 << 7
                            | 2 << 10
                            | 2 << 13
                            | 2 << 16
                            | swizzle[0] << 19
                            | swizzle[1] << 22
                            | swizzle[2] << 25
                            | swizzle[3] << 28,
                        0x200c00,
                        header << 21,
                        if header == 2 {
                            1
                        } else if mip_texture.is_some() {
                            1 << 28
                        } else {
                            0
                        },
                        1 | 1 << 23 | 7 << 29 | if format == 4 { 1 << 22 } else { 0 },
                        1 | 1 << 31,
                        0,
                        if mip_texture.is_some() { 1 << 4 } else { 0 },
                    ];
                    let (mip_filter, lod) = match mip_texture {
                        Some((true, ..)) => (3, 128),
                        Some((false, ..)) => (2, 256),
                        None => (1, 0),
                    };
                    let tsc: [u32; 8] = [
                        2 | 2 << 3 | 2 << 6 | 1 << 13 | 1 << 14 | 1 << 17,
                        2 | 2 << 4 | mip_filter << 6 | 1 << 8 | 1 << 9,
                        lod | lod << 12,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ];
                    memory
                        .write(
                            0xc460,
                            &tic.into_iter()
                                .flat_map(u32::to_le_bytes)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap();
                    memory
                        .write(
                            0xc480,
                            &tsc.into_iter()
                                .flat_map(u32::to_le_bytes)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap();
                    let last = if refresh { [0, 0, 0, 255] } else { [255; 4] };
                    let rgba = if format == 4 {
                        srgb_texture_pixels(refresh).concat()
                    } else {
                        [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], last].concat()
                    };
                    let packed: Vec<_> = if format >= 5 {
                        compressed_texture_block(format, refresh)
                    } else {
                        rgba.chunks_exact(4)
                            .flat_map(|pixel| pixel[..components].iter().copied())
                            .collect()
                    };
                    // Compressed footprints use block rows, not texel rows.
                    let row_bytes = match format {
                        5 => 8,
                        6 | 7 => 16,
                        _ => 2 * components,
                    };
                    let rows = if format >= 5 { 1 } else { 2 };
                    let mut payload = vec![0xcc; if mip_texture.is_some() { 1024 } else { 512 }];
                    if header == 2 {
                        payload[..row_bytes].copy_from_slice(&packed[..row_bytes]);
                        if rows == 2 {
                            payload[32..32 + row_bytes].copy_from_slice(&packed[row_bytes..]);
                        }
                    } else {
                        exosphere_gpu_ir::block_linear::swizzle_2d_into(
                            &packed,
                            &mut payload,
                            exosphere_gpu_ir::BlockLinearLayout::new(
                                row_bytes as u32,
                                rows,
                                1,
                                0,
                                0,
                            ),
                        )
                        .unwrap();
                    }
                    if mip_texture.is_some() {
                        // Mip 1 starts at 0xce00. Completion at 0xcf00 lies in
                        // unused GOB padding, never in a sampled logical texel.
                        let mip = if format == 5 {
                            let endpoint = if refresh { 0x07e0u16 } else { 0xf800u16 };
                            let mut block = vec![0; 8];
                            block[..2].copy_from_slice(&endpoint.to_le_bytes());
                            block
                        } else if refresh {
                            vec![71, 89, 113, 255]
                        } else {
                            vec![17, 43, 91, 255]
                        };
                        exosphere_gpu_ir::block_linear::swizzle_2d_into(
                            &mip,
                            &mut payload[512..],
                            exosphere_gpu_ir::BlockLinearLayout::new(
                                if format == 5 { 8 } else { 4 },
                                1,
                                1,
                                0,
                                0,
                            ),
                        )
                        .unwrap();
                    }
                    memory.write(0xcc00, &payload).unwrap();
                }
                memory.write(0xc500, &depth).unwrap();
                memory.write(0xc504, &2.0f32.to_le_bytes()).unwrap();
                let indices = if vertex_id {
                    vec![99, 0, 1, 0, 2, 0, 3, 0]
                } else if restart {
                    vec![99, 0, 0, 0, 1, 0, 2, 0, 255, 255, 0, 0, 1, 0, 2, 0]
                } else {
                    vec![99, 0, 0, 0, 1, 0, 2, 0]
                };
                memory.write(0xc400, &indices).unwrap();
                memory.write(0xcf00, &[0; 4]).unwrap();
                let background: Vec<_> = [17, 43, 91, 255].into_iter().cycle().take(4096).collect();
                exosphere_runtime::gpu::color_memory::write_color_layer(
                    memory,
                    gpu,
                    &state,
                    0,
                    TextureFormat::Rgba8Unorm,
                    &background,
                )
                .unwrap();
            })
            .unwrap();
        let mut words = vec![0x60010000u32, 0xb197];
        for (&register, &word) in &state.raw_registers {
            words.extend([0x60010000 | u32::from(register), word]);
        }
        if vertex_id {
            for (register, value) in [
                (method::SET_VERTEX_ID_REPLACE, 0),
                (method::SET_VERTEX_ID_GEN_MODE, 0),
                (
                    method::SET_VERTEX_ID_BASE,
                    if mode == 10 {
                        0x40000000
                    } else {
                        u32::from_le_bytes(depth).wrapping_sub(1)
                    },
                ),
            ] {
                words.extend([0x60010000 | u32::from(register), value]);
            }
        }
        if instanced {
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
                words.extend([0x60010000 | u32::from(register), value]);
            }
        }
        for (&(group, _slot), binding) in &state.constant_buffers {
            for (register, value) in [
                (method::CONSTANT_BUFFER_SELECTOR_SIZE, binding.size_raw),
                (method::CONSTANT_BUFFER_SELECTOR_HIGH, binding.address.a),
                (method::CONSTANT_BUFFER_SELECTOR_LOW, binding.address.b),
                (
                    method::CONSTANT_BUFFER_BIND_BASE
                        + u16::from(group) * method::CONSTANT_BUFFER_BIND_STRIDE,
                    binding.bind_raw,
                ),
            ] {
                words.extend([0x60010000 | u32::from(register), value]);
            }
        }
        if shift.is_some() || extract.is_some() || pop_count.is_some() || leading.is_some() {
            for (register, value) in [
                // Uniform blocks retain their real 16-byte allocation; no
                // implicit padding is supplied by the descriptor translator.
                (method::CONSTANT_BUFFER_SELECTOR_SIZE, 16),
                (method::CONSTANT_BUFFER_SELECTOR_HIGH, 0),
                (method::CONSTANT_BUFFER_SELECTOR_LOW, 0x200d00),
                (
                    method::CONSTANT_BUFFER_BIND_BASE + 4 * method::CONSTANT_BUFFER_BIND_STRIDE,
                    0x11,
                ),
            ] {
                words.extend([0x60010000 | u32::from(register), value]);
            }
        }
        if texture.is_some() {
            for (register, value) in [
                (method::SET_BINDLESS_TEXTURE, 1),
                (method::SET_LINKED_TSC, 0),
                (method::SET_TEX_HEADER_POOL_A, 0),
                (method::SET_TEX_HEADER_POOL_B, 0x200460),
                (method::SET_TEX_HEADER_POOL_MAXIMUM_INDEX, 0),
                (method::SET_TEX_SAMPLER_POOL_A, 0),
                (method::SET_TEX_SAMPLER_POOL_B, 0x200480),
                (method::SET_TEX_SAMPLER_POOL_MAXIMUM_INDEX, 0),
                (method::CONSTANT_BUFFER_SELECTOR_SIZE, 4),
                (method::CONSTANT_BUFFER_SELECTOR_HIGH, 0),
                (method::CONSTANT_BUFFER_SELECTOR_LOW, 0x200440),
                (
                    method::CONSTANT_BUFFER_BIND_BASE + 4 * method::CONSTANT_BUFFER_BIND_STRIDE,
                    0x11,
                ),
            ] {
                words.extend([0x60010000 | u32::from(register), value]);
            }
        }
        let draw_words = if indexed {
            vec![
                (
                    method::SET_VERTEX_ELEMENT_BASE,
                    if vertex_id { u32::MAX } else { 0 },
                ),
                (method::SET_PRIMITIVE_RESTART, u32::from(restart)),
                (method::SET_PRIMITIVE_RESTART_INDEX, 0xffff),
                (method::SET_INDEX_BUFFER_A, 0),
                (method::SET_INDEX_BUFFER_B, 0x200400),
                (method::SET_INDEX_BUFFER_C, 0),
                (
                    method::SET_INDEX_BUFFER_D,
                    if restart { 0x20040f } else { 0x200407 },
                ),
                (method::SET_INDEX_BUFFER_E, 1),
                // Skip a sentinel: FIRST must enter the binding exactly once.
                (method::SET_INDEX_BUFFER_F, 1),
                (
                    method::BEGIN,
                    if restart {
                        5
                    } else {
                        state.primitive_raw.unwrap() | instance_flags
                    },
                ),
                (method::DRAW_INDEX_BUFFER, if restart { 7 } else { count }),
            ]
        } else {
            vec![
                (method::BEGIN, state.primitive_raw.unwrap()),
                (method::SET_VERTEX_ARRAY_START, first),
                (method::DRAW_VERTEX_ARRAY, count),
            ]
        };
        for (register, value) in draw_words {
            words.extend([0x60010000 | u32::from(register), value]);
        }
        words.extend([0x20040004, 0, 0x101f00, 42, 0x01000002]);
        let trace = admission_front.process_pushbuffer(&words).unwrap();
        if instanced {
            let expected_instance = match mode {
                3 | 8 => 0,
                4 => 1,
                5 | 6 => 2,
                7 => 3,
                _ => unreachable!(),
            };
            let captured_instance = trace.graphics_events.iter().find_map(|event| match event {
                exosphere_gpu_ir::MaxwellEvent::DrawIndexBuffer { state, .. } => {
                    state.draw_instance_id()
                }
                _ => None,
            });
            assert_eq!(captured_instance, Some(expected_instance));
        }
        let opaque: Vec<_> = trace
            .graphics_events
            .iter()
            .filter(|event| matches!(event, exosphere_gpu_ir::MaxwellEvent::OpaqueRegister { .. }))
            .collect();
        assert!(
            opaque.is_empty(),
            "fixture contains unclassified state writes: {opaque:?}"
        );
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        assert!(bytes.len() <= 0x600); // Ends before the sampled image at 0xcc00.
        driver
            .kernel
            .process_mut(driver.process)
            .unwrap()
            .memory
            .write(0xc600, &bytes)
            .unwrap();
        let entry = 0x101600 | ((words.len() as u64) << 42);
        let mut submit = [0; 32];
        submit[8..12].copy_from_slice(&1u32.to_le_bytes());
        submit[12..16].copy_from_slice(&4u32.to_le_bytes());
        submit[24..32].copy_from_slice(&entry.to_le_bytes());
        last_submission = submit;
        driver.ioctl_buffer(channel, 0xc0204808, &submit, true);
        let mut semaphore = [0; 4];
        driver
            .kernel
            .process(driver.process)
            .unwrap()
            .memory
            .read(0xcf00, &mut semaphore)
            .unwrap();
        assert_eq!(semaphore, [0; 4], "admission must not signal completion");
        assert_eq!(
            driver
                .kernel
                .poll_gpu_process(driver.process, 1)
                .unwrap_or_else(|error| panic!("fixture mode {mode}: {error:?}")),
            1
        );
        driver
            .kernel
            .with_gpu_channel(driver.process, driver.session, channel, |_, memory, gpu| {
                let actual = exosphere_runtime::gpu::color_memory::read_color_layer(
                    memory,
                    gpu,
                    &state,
                    0,
                    TextureFormat::Rgba8Unorm,
                )
                .unwrap();
                if mip_texture.is_some()
                    || texture.is_some_and(|(_, _, _, format)| (4..=7).contains(&format))
                {
                    for (actual, reference) in actual.chunks_exact(4).zip(expected.chunks_exact(4))
                    {
                        if reference == [17, 43, 91, 255] {
                            assert_eq!(actual, reference);
                        } else {
                            assert!(
                                actual[..3]
                                    .iter()
                                    .zip(&reference[..3])
                                    .all(|(a, b)| a.abs_diff(*b) <= 1),
                                "filtered texture mode {mode}: {actual:?} vs {reference:?}"
                            );
                            assert_eq!(actual[3], reference[3], "texture alpha must match exactly");
                        }
                    }
                } else {
                    assert_eq!(actual, expected);
                }
                memory.read(0xcf00, &mut semaphore).unwrap();
                assert_eq!(u32::from_le_bytes(semaphore), 42);
            })
            .unwrap();
        assert_eq!(
            driver.kernel.poll_gpu_process(driver.process, 1).unwrap(),
            0
        );
    }
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .write(0xc350, &saved_fragment_body)
        .unwrap();
    let mut saved_instruction = [0; 8];
    let mut saved_target = vec![0; PAGE_SIZE as usize];
    {
        let memory = &mut driver.kernel.process_mut(driver.process).unwrap().memory;
        memory.read(0xc158, &mut saved_instruction).unwrap();
        memory.read(0xb000, &mut saved_target).unwrap();
        // An unknown instruction must fail metadata admission before Vulkan work.
        memory.write(0xc158, &u64::MAX.to_le_bytes()).unwrap();
        memory.write(0xcf00, &[0; 4]).unwrap();
    }
    driver.ioctl_buffer(channel, 0xc0204808, &last_submission, true);
    assert!(driver.kernel.poll_gpu_process(driver.process, 1).is_err());
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .write(0xc158, &saved_instruction)
        .unwrap();
    // Repairing memory does not implicitly replay a descriptor on a faulted queue.
    assert!(driver.kernel.poll_gpu_process(driver.process, 1).is_err());
    let memory = &driver.kernel.process(driver.process).unwrap().memory;
    let mut actual = vec![0; saved_target.len()];
    memory.read(0xb000, &mut actual).unwrap();
    assert_eq!(actual, saved_target);
    let mut semaphore = [0; 4];
    memory.read(0xcf00, &mut semaphore).unwrap();
    assert_eq!(semaphore, [0; 4]);
    driver.close(channel);
    assert_eq!(*closed.lock().unwrap(), vec![owner]);
    driver
        .kernel
        .close_handle(driver.process, driver.session)
        .unwrap();
    assert_eq!(closed.lock().unwrap().len(), 1);
}

#[test]
fn direct_provider_reads_live_authenticated_programs_and_observes_native_unmap() {
    use exosphere_gpu_ir::maxwell_state::method;
    use exosphere_gpu_ir::shader_extent::ShaderExtentError;
    use exosphere_runtime::gpu::shader_memory::DirectDrawShaderGroups;

    let mut driver = Driver::new();
    let (map, space, channel) = driver.devices();
    let created = driver.ioctl_buffer(
        map,
        0xc0080101,
        &[(PAGE_SIZE as u32).to_le_bytes(), 0u32.to_le_bytes()].concat(),
        true,
    );
    let handle = u32::from_le_bytes(created[4..8].try_into().unwrap());
    let mut allocation = [0; 32];
    allocation[..4].copy_from_slice(&handle.to_le_bytes());
    allocation[8..12].copy_from_slice(&1u32.to_le_bytes());
    allocation[24..32].copy_from_slice(&0x8000u64.to_le_bytes());
    driver.ioctl_buffer(map, 0xc0200104, &allocation, true);
    let mut reservation = [0; 24];
    reservation[..4].copy_from_slice(&1u32.to_le_bytes());
    reservation[4..8].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    reservation[8..12].copy_from_slice(&1u32.to_le_bytes());
    reservation[16..24].copy_from_slice(&0x100000u64.to_le_bytes());
    driver.ioctl_buffer(space, 0xc0184102, &reservation, true);
    let mut mapping = [0; 40];
    mapping[..4].copy_from_slice(&1u32.to_le_bytes());
    mapping[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    mapping[8..12].copy_from_slice(&handle.to_le_bytes());
    mapping[12..16].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    mapping[24..32].copy_from_slice(&PAGE_SIZE.to_le_bytes());
    mapping[32..40].copy_from_slice(&0x100000u64.to_le_bytes());
    driver.ioctl_buffer(space, 0xc0284106, &mapping, true);

    let mut state = DrawStateSnapshot::default();
    state
        .raw_registers
        .insert(method::SET_PROGRAM_REGION_B, 0x100000);
    for (slot, select, offset) in [(1, 0x11, 0x200), (5, 0x51, 0x400)] {
        let base = method::PIPELINE_BASE + slot * method::PIPELINE_STRIDE;
        state.raw_registers.insert(base, select);
        state.raw_registers.insert(base + 1, offset);
        let mut bytes = vec![0; 88]; // Graphics header followed by one scheduling word.
        bytes.extend(0xe30000000007000fu64.to_le_bytes());
        driver
            .kernel
            .process_mut(driver.process)
            .unwrap()
            .memory
            .write(0x8000 + u64::from(offset), &bytes)
            .unwrap();
    }
    let mut provider = DirectDrawProgramProvider::new(
        4096,
        DirectDrawShaderGroups {
            vertex_group: 0,
            fragment_group: 4,
            vertex_set: 0,
            fragment_set: 1,
        },
    )
    .unwrap();
    let resolve = |driver: &mut Driver, provider: &mut DirectDrawProgramProvider| {
        driver
            .kernel
            .with_gpu_channel(
                driver.process,
                driver.session,
                channel,
                |identity, memory, gpu| provider.resolve(&identity, memory, gpu, &state),
            )
            .unwrap()
    };
    let programs = resolve(&mut driver, &mut provider).unwrap();
    assert_eq!(
        programs
            .windows
            .iter()
            .map(|window| (window.gpu_address, window.bytes))
            .collect::<Vec<_>>(),
        vec![(0x100200, 96), (0x100400, 96)]
    );

    // Replace the first EXIT in place with NOP and append a later EXIT.
    // The same provider/address must discover the new extent, not reuse metadata.
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .write(
            0x8258,
            &[0u64.to_le_bytes(), 0xe30000000007000fu64.to_le_bytes()].concat(),
        )
        .unwrap();
    let programs = resolve(&mut driver, &mut provider).unwrap();
    assert_eq!(programs.windows[0].bytes, 104);
    assert_eq!(programs.windows[1].bytes, 96);

    driver.ioctl_buffer(space, 0xc0084105, &0x100000u64.to_le_bytes(), true);
    assert!(matches!(
        resolve(&mut driver, &mut provider),
        Err(ColorReadbackError::ShaderExtent(ShaderExtentError::Memory(
            _
        )))
    ));
}

impl DrawProgramProvider for CleanupProbe {
    fn resolve(
        &mut self,
        _: &NvGpuChannelIdentity,
        _: &AddressSpace,
        _: &NvGpuAddressSpace,
        _: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError> {
        panic!("closing an unused channel must not resolve shader metadata");
    }

    fn close_channel(&mut self, identity: &NvGpuChannelIdentity) {
        self.0.lock().unwrap().push(identity.clone());
    }
}

#[test]
fn installed_linux_adapter_receives_authenticated_ipc_close() {
    use exosphere_runtime::gpu::nv_backend::NativeGpuBackend;

    let mut driver = Driver::new();
    let closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let renderer = LinuxDrawRenderer::new(CleanupProbe(closed.clone()), 2).unwrap();
    driver
        .kernel
        .install_gpu_ioctl_backend(Box::new(
            NativeGpuBackend::with_renderer(2, Box::new(renderer)).unwrap(),
        ))
        .unwrap();
    let first = driver.channel();
    let second = driver.channel();
    let identity = |driver: &mut Driver, descriptor| {
        driver
            .kernel
            .with_gpu_channel(driver.process, driver.session, descriptor, |owner, _, _| {
                owner
            })
            .unwrap()
    };
    let first_owner = identity(&mut driver, first);
    let second_owner = identity(&mut driver, second);
    assert_ne!(first_owner, second_owner);
    assert!(closed.lock().unwrap().is_empty());

    driver.close(first);
    assert_eq!(*closed.lock().unwrap(), vec![first_owner]);
    assert!(
        driver
            .kernel
            .with_gpu_channel(driver.process, driver.session, first, |_, _, _| (),)
            .is_err()
    );
    // Closing one channel must leave the other canonical binding intact.
    assert_eq!(identity(&mut driver, second), second_owner);
    driver.close(second);
    assert_eq!(closed.lock().unwrap().len(), 2);
    assert_eq!(closed.lock().unwrap()[1], second_owner);
    driver
        .kernel
        .close_handle(driver.process, driver.session)
        .unwrap();
    assert_eq!(closed.lock().unwrap().len(), 2);
}

#[derive(Debug, Default)]
struct RejectPrograms {
    resolved: Vec<NvGpuChannelIdentity>,
    closed: Vec<NvGpuChannelIdentity>,
}

#[test]
fn installed_linux_adapter_cleans_live_channels_on_session_close_and_process_exit() {
    use exosphere_runtime::gpu::nv_backend::NativeGpuBackend;

    for exit_process in [false, true] {
        let mut driver = Driver::new();
        let closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let renderer = LinuxDrawRenderer::new(CleanupProbe(closed.clone()), 2).unwrap();
        driver
            .kernel
            .install_gpu_ioctl_backend(Box::new(
                NativeGpuBackend::with_renderer(2, Box::new(renderer)).unwrap(),
            ))
            .unwrap();
        let mut owners = Vec::new();
        for _ in 0..2 {
            let channel = driver.channel();
            owners.push(
                driver
                    .kernel
                    .with_gpu_channel(driver.process, driver.session, channel, |owner, _, _| owner)
                    .unwrap(),
            );
        }
        assert!(closed.lock().unwrap().is_empty());
        if exit_process {
            driver.kernel.exit_process(driver.process).unwrap();
        } else {
            driver
                .kernel
                .close_handle(driver.process, driver.session)
                .unwrap();
        }
        let actual = closed.lock().unwrap().clone();
        assert_eq!(actual.len(), owners.len());
        for owner in &owners {
            assert_eq!(
                actual
                    .iter()
                    .filter(|candidate| *candidate == owner)
                    .count(),
                1
            );
        }
        // Process teardown after session teardown must not close channels twice.
        driver.kernel.exit_process(driver.process).unwrap();
        assert_eq!(*closed.lock().unwrap(), actual);
    }
}

impl DrawProgramProvider for RejectPrograms {
    fn resolve(
        &mut self,
        identity: &NvGpuChannelIdentity,
        _: &AddressSpace,
        _: &NvGpuAddressSpace,
        _: &DrawStateSnapshot,
    ) -> Result<VerifiedDrawPrograms, ColorReadbackError> {
        self.resolved.push(identity.clone());
        Err(ColorReadbackError::UnsupportedDraw)
    }

    fn close_channel(&mut self, identity: &NvGpuChannelIdentity) {
        if !self.closed.contains(identity) {
            self.closed.push(identity.clone());
        }
    }
}

#[test]
fn authenticated_program_rejection_preserves_channel_identity_without_native_allocation() {
    let mut renderer = LinuxDrawRenderer::new(RejectPrograms::default(), 1).unwrap();
    let mut owners = Vec::new();
    for _ in 0..2 {
        let mut driver = Driver::new();
        let channel = driver.channel();
        driver
            .kernel
            .with_gpu_channel(
                driver.process,
                driver.session,
                channel,
                |identity, memory, gpu| {
                    let event = exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
                        first: 0,
                        count: 3,
                        state: Default::default(),
                    };
                    let error = renderer.draw(&identity, memory, gpu, &event).unwrap_err();
                    let exosphere_runtime::gpu::GuestGpuExecutionFailure::Draw(source) = error
                    else {
                        panic!("adapter lost the Linux error source");
                    };
                    assert!(matches!(
                        source.downcast_ref::<ColorReadbackError>(),
                        Some(ColorReadbackError::UnsupportedDraw)
                    ));
                    owners.push(identity);
                },
            )
            .unwrap();
    }
    // Equal descriptor numbers in separate sessions must not alias owners.
    assert_ne!(owners[0], owners[1]);
    assert_eq!(renderer.provider.resolved, owners);
    assert!(renderer.channels.is_empty());
    for owner in &owners {
        renderer.close_channel(owner);
        renderer.close_channel(owner);
    }
    assert_eq!(renderer.provider.closed, owners);
    assert!(renderer.channels.is_empty());
}
