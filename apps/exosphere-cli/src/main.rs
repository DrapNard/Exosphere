// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Exosphere command-line frontend.

use clap::{Parser, Subcommand};
mod content;
use exosphere_ingest::InputKind;
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(
    author = "DrapNard",
    version,
    about = "Exosphere — Switch compatibility compiler/runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Doctor,
    Inspect {
        input: PathBuf,
        #[arg(long, default_value_t = 0)]
        decode: usize,
    },
    ContentInspect {
        input: PathBuf,
        #[arg(long)]
        keys: PathBuf,
        #[arg(long, default_value_t = 0)]
        cpu_steps: usize,
    },
    CpuIr {
        input: PathBuf,
        #[arg(long)]
        llvm: bool,
    },
    HorizonSmoke,
    HorizonIpcSmoke,
    FsSmoke,
    GpuTriangle {
        #[arg(long, default_value_t = 1280)]
        width: u32,
        #[arg(long, default_value_t = 720)]
        height: u32,
    },
    VulkanSmoke,
    VulkanTriangleSmoke {
        #[arg(long, default_value_t = 256)]
        width: u32,
        #[arg(long, default_value_t = 256)]
        height: u32,
    },
    VulkanAshTriangleSmoke {
        #[arg(long, default_value_t = 256)]
        width: u32,
        #[arg(long, default_value_t = 256)]
        height: u32,
    },
    MaxwellShaderInspect {
        input: PathBuf,
        #[arg(long, default_value = "vertex")]
        stage: String,
    },
    PfifoInspect {
        input: PathBuf,
    },
    MaxwellFrontendInspect {
        input: PathBuf,
    },
    GpuAddressSmoke,
    TextureFootprint {
        #[arg(long)]
        width: u32,
        #[arg(long)]
        height: u32,
        #[arg(long, default_value = "rgba8")]
        format: String,
    },
    BlockLinearSmoke {
        #[arg(long, default_value_t = 256)]
        width_bytes: u32,
        #[arg(long, default_value_t = 64)]
        height_rows: u32,
        #[arg(long, default_value_t = 4)]
        block_height: u8,
    },
    DescriptorInspect {
        input: PathBuf,
        #[arg(long, default_value = "tic")]
        kind: String,
    },
    MmeSmoke,
    GpuCapabilities,
    GpuTransferSmoke,
    EnvydisOracle {
        input: PathBuf,
        #[arg(long, default_value = "envydis")]
        program: PathBuf,
    },
    SaveJournalSmoke {
        directory: PathBuf,
    },
    Run {
        package: String,
    },
}

fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = Cli::parse();
    if let Err(error) = dispatch(cli.command) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn dispatch(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Doctor => {
            match exosphere_platform_linux::create_default_runtime() {
                Ok(runtime) => println!(
                    "Exosphere Linux runtime ready: hypervisor={}",
                    runtime.hypervisor_name()
                ),
                Err(error) => println!("Exosphere Linux runtime: KVM unavailable ({error})"),
            }
            println!("Vulkan loader: {:?}", exosphere_gpu_vulkan::loader::probe());
        }
        Command::Inspect { input, decode } => inspect(input, decode)?,
        Command::ContentInspect {
            input,
            keys,
            cpu_steps,
        } => content::inspect(input, keys, cpu_steps)?,
        Command::CpuIr { input, llvm } => cpu_ir(input, llvm)?,
        Command::HorizonSmoke => horizon_smoke()?,
        Command::HorizonIpcSmoke => horizon_ipc_smoke()?,
        Command::FsSmoke => fs_smoke()?,
        Command::GpuTriangle { width, height } => gpu_triangle(width, height)?,
        Command::VulkanSmoke => vulkan_smoke()?,
        Command::VulkanTriangleSmoke { width, height } => vulkan_triangle_smoke(width, height)?,
        Command::VulkanAshTriangleSmoke { width, height } => {
            vulkan_ash_triangle_smoke(width, height)?
        }
        Command::MaxwellShaderInspect { input, stage } => maxwell_shader_inspect(input, &stage)?,
        Command::PfifoInspect { input } => pfifo_inspect(input)?,
        Command::MaxwellFrontendInspect { input } => maxwell_frontend_inspect(input)?,
        Command::GpuAddressSmoke => gpu_address_smoke()?,
        Command::TextureFootprint {
            width,
            height,
            format,
        } => texture_footprint(width, height, &format)?,
        Command::BlockLinearSmoke {
            width_bytes,
            height_rows,
            block_height,
        } => block_linear_smoke(width_bytes, height_rows, block_height)?,
        Command::DescriptorInspect { input, kind } => descriptor_inspect(input, &kind)?,
        Command::MmeSmoke => mme_smoke()?,
        Command::GpuCapabilities => gpu_capabilities()?,
        Command::GpuTransferSmoke => gpu_transfer_smoke()?,
        Command::EnvydisOracle { input, program } => envydis_oracle(input, program)?,
        Command::SaveJournalSmoke { directory } => save_journal_smoke(directory)?,
        Command::Run { package } => {
            println!("launch request: {package} (full scheduler/GPU execution not wired yet)")
        }
    }
    Ok(())
}

fn horizon_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_horizon::{
        HorizonKernel, HorizonProfileId,
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        svc::{SvcArguments, dispatch},
    };
    let mut kernel = HorizonKernel::new(HorizonProfileId("generic".into()));
    let pid = kernel.create_process(0x0100_0000_0000_0000, "exosphere-smoke");
    kernel.process_mut(pid)?.memory.map(
        0x0071_0000_0000,
        PAGE_SIZE,
        MemoryPermission::READ_EXECUTE,
        MemoryKind::Code,
    )?;
    let (tid, handle) = kernel.create_thread(pid, 0x0071_0000_0000, 0x7fff_ffff_0000, 0x2c)?;
    kernel.start_thread(tid)?;
    let result = dispatch(&mut kernel, pid, tid, 0x25, SvcArguments::default())?;
    println!(
        "Horizon smoke OK: pid={pid} tid={} handle=0x{:08x} svcGetThreadId={}",
        tid, handle.0, result.x[1]
    );
    Ok(())
}

fn horizon_ipc_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_horizon::{HorizonKernel, HorizonProfileId, cmif, hipc};
    let mut kernel = HorizonKernel::new(HorizonProfileId("generic".into()));
    let pid = kernel.create_process(0x0100_0000_0000_0000, "ipc-smoke");
    let (tid, _) = kernel.create_thread(pid, 0x1000, 0x8000, 0x2c)?;
    let session = kernel.connect_service(pid, "sm:")?;

    // HIPC raw-data body: SFCI Initialize command followed by 16 bytes of mandatory total padding.
    let mut cmif_request = Vec::new();
    cmif_request.extend_from_slice(b"SFCI");
    cmif_request.extend_from_slice(&1u16.to_le_bytes());
    cmif_request.extend_from_slice(&0u16.to_le_bytes());
    cmif_request.extend_from_slice(&0u32.to_le_bytes());
    cmif_request.extend_from_slice(&0u32.to_le_bytes());
    cmif_request.extend_from_slice(&[0u8; 16]);
    let raw_words = (cmif_request.len() / 4) as u32;
    let mut request = Vec::new();
    request.extend_from_slice(&1u32.to_le_bytes());
    request.extend_from_slice(&raw_words.to_le_bytes());
    request.extend_from_slice(&cmif_request);
    kernel.thread_mut(tid)?.ipc_buffer = request;
    kernel.send_sync_request(pid, tid, session)?;

    let response = hipc::parse(&kernel.thread(tid)?.ipc_buffer)?;
    let response = cmif::parse_response(response.raw_data)?;
    println!(
        "HIPC/CMIF smoke OK: result=0x{:08x} version={}",
        response.header.result, response.header.version
    );
    Ok(())
}

fn fs_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_horizon::{HorizonKernel, HorizonProfileId};
    use exosphere_vfs::{GuestPath, backend::memory::MemoryFileSystem};
    use std::sync::Arc;
    let mut kernel = HorizonKernel::new(HorizonProfileId("generic".into()));
    let rom = Arc::new(MemoryFileSystem::new(true));
    rom.insert_read_only_file(
        &GuestPath::parse("/hello.txt")?,
        b"hello from Exosphere".to_vec(),
    );
    kernel.fs.mount("rom", rom, false);
    let mut bytes = vec![0u8; 64];
    let count = kernel.fs.read("rom:/hello.txt", 0, &mut bytes)?;
    println!("VFS smoke OK: {}", String::from_utf8_lossy(&bytes[..count]));
    Ok(())
}

fn gpu_triangle(width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
    let module = exosphere_gpu_ir::GpuModule::triangle_smoke_test(width, height);
    let backend = exosphere_gpu_vulkan::VulkanBackend::new();
    let plan = backend.plan(&module)?;
    println!(
        "Vulkan triangle plan: {} pipelines, {} shaders, {} commands",
        plan.graphics_pipelines.len(),
        plan.shaders.len(),
        plan.commands.len()
    );
    for shader in &plan.shaders {
        println!(
            "  shader {} {:?}: {} SPIR-V words",
            shader.id,
            shader.stage,
            shader.spirv.words.len()
        );
    }
    for command in plan.commands {
        println!("  {command:?}");
    }
    Ok(())
}

fn inspect(path: PathBuf, decode: usize) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut input = fs::File::open(&path)?;
    let mut magic = [0u8; 4];
    input.read_exact(&mut magic)?;
    if &magic == b"PFS0" {
        let index = exosphere_ingest::pfs0::read_index(&mut input)?;
        println!(
            "file: {}\nsize: {} bytes\nkind: Pfs0",
            path.display(),
            input.metadata()?.len()
        );
        for entry in index.entries {
            println!("{:>10} {}", entry.size, entry.name);
        }
        return Ok(());
    }
    let bytes = fs::read(&path)?;
    let kind = exosphere_ingest::identify(&bytes);
    println!(
        "file: {}\nsize: {} bytes\nkind: {kind:?}",
        path.display(),
        bytes.len()
    );
    match kind {
        InputKind::Nro => {
            let n = exosphere_ingest::nro::parse(&bytes)?;
            println!("module-id: {}", hex(&n.header.module_id));
            if decode > 0 {
                for instruction in exosphere_cpu_ir::decode::decode_text(
                    n.text,
                    n.header.text.memory_offset as u64,
                )
                .into_iter()
                .take(decode)
                {
                    println!(
                        "0x{:016x}: {:08x} {:?}",
                        instruction.guest_pc, instruction.raw, instruction.opcode
                    );
                }
            }
        }
        InputKind::Nso => {
            let n = exosphere_ingest::nso::load(&bytes)?;
            println!(
                "module-id: {} text={} rodata={} data={}",
                hex(&n.header.module_id),
                n.text.len(),
                n.rodata.len(),
                n.data.len()
            );
        }
        InputKind::Pfs0 => {
            for entry in exosphere_ingest::pfs0::parse(&bytes)?.entries {
                println!("{:>10} {}", entry.size, entry.name);
            }
        }
        InputKind::Hfs0 => {
            for entry in exosphere_ingest::hfs0::parse(&bytes)?.entries {
                println!("{:>10} {}", entry.size, entry.name);
            }
        }
        InputKind::Xci => {
            let x = exosphere_ingest::xci::parse(&bytes)?;
            println!("root HFS0 @ 0x{:x}", x.root_offset);
            for entry in x.root.entries {
                println!("{:>10} {}", entry.size, entry.name);
            }
        }
        InputKind::RomFs => {
            for file in exosphere_ingest::romfs::parse(&bytes)?.files {
                println!("{:>10} {}", file.size, file.path);
            }
        }
        InputKind::Nca => {
            let n = exosphere_ingest::nca::parse_plaintext_header(&bytes)?;
            println!(
                "NCA {:?} program={:016x} size={}",
                std::str::from_utf8(&n.magic).unwrap_or("????"),
                n.program_id,
                n.content_size
            );
        }
        _ => println!("no structural parser for this input yet"),
    }
    Ok(())
}

fn cpu_ir(path: PathBuf, llvm: bool) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let (text, base) = match exosphere_ingest::identify(&bytes) {
        InputKind::Nro => {
            let n = exosphere_ingest::nro::parse(&bytes)?;
            (n.text.to_vec(), n.header.text.memory_offset as u64)
        }
        InputKind::Nso => {
            let n = exosphere_ingest::nso::load(&bytes)?;
            (n.text, n.header.text.memory_offset as u64)
        }
        _ => return Err("CPU IR input must be NRO or NSO".into()),
    };
    let decoded = exosphere_cpu_ir::decode::decode_text(&text, base);
    let blocks = exosphere_cpu_ir::cfg::build(&decoded);
    let function = exosphere_cpu_ir::lift::lift(base, &blocks);
    let stats = exosphere_cpu_backend_cranelift::validate_lowering(&function);
    println!(
        "blocks={} native-ops={} fallback-ops={}",
        function.blocks.len(),
        stats.native_ops,
        stats.fallback_ops
    );
    if llvm {
        print!("{}", exosphere_cpu_backend_llvm::emit_llvm_ir(&function));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn vulkan_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let instance = exosphere_gpu_vulkan::raw_instance::RawVulkanInstance::create()?;
    instance.submit_empty_command_buffer()?;
    let (major, minor, patch) = instance.api_version_tuple();
    println!(
        "Vulkan smoke OK: loader API {major}.{minor}.{patch}, physical-devices={}, logical-device={}, graphics-queue={}, queue-family={:?}, command-submit=true",
        instance.physical_device_count(),
        instance.has_logical_device(),
        instance.has_graphics_queue(),
        instance.queue_family_index()
    );
    Ok(())
}

fn vulkan_triangle_smoke(width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
    let module = exosphere_gpu_ir::GpuModule::triangle_smoke_test(width, height);
    let backend = exosphere_gpu_vulkan::VulkanBackend::new();
    let plan = backend.plan(&module)?;
    let instance = exosphere_gpu_vulkan::raw_instance::RawVulkanInstance::create()?;
    let report = instance.execute_offscreen_plan(&plan)?;
    println!(
        "Vulkan real draw OK: {}x{}, vertices={}, shader-modules={}, bright-pixels={}, target=offscreen RGBA8",
        report.width, report.height, report.vertices, report.shader_modules, report.bright_pixels
    );
    Ok(())
}

fn vulkan_ash_triangle_smoke(width: u32, height: u32) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_vulkan::device::VulkanDevice;
    if width < 8 || height < 8 {
        return Err("triangle pixel validation requires at least 8x8".into());
    }
    let module = exosphere_gpu_ir::GpuModule::triangle_vulkan_smoke_test(width, height);
    let mut resources = exosphere_gpu_ir::InMemoryGpuResources::new();
    println!("ash Vulkan triangle: creating backend");
    let mut device = exosphere_gpu_vulkan::ash_device::AshVulkanDevice::create()?;
    let plan = exosphere_gpu_vulkan::VulkanBackend::new().plan(&module)?;
    let mut rejected_plan = plan.clone();
    let rendering = rejected_plan
        .commands
        .iter()
        .position(|command| {
            matches!(
                command,
                exosphere_gpu_vulkan::plan::VulkanPlanCommand::BeginRendering { .. }
            )
        })
        .ok_or("smoke plan has no rendering begin")?;
    rejected_plan.commands.insert(
        rendering + 1,
        exosphere_gpu_vulkan::plan::VulkanPlanCommand::ClearColor {
            target: module.textures[0].id,
            value: [0; 4],
        },
    );
    // Cycle beyond the frame ring: rejected recording must not strand a fence
    // or retain image layouts from commands that were never submitted.
    for _ in 0..4 {
        match device.execute_plan_with_resources(&rejected_plan, &mut resources) {
            Err(exosphere_gpu_vulkan::VulkanError::UnsupportedFeature(
                "clear image during rendering; use attachment clear lowering",
            )) => {}
            other => return Err(format!("unexpected recording-rejection result: {other:?}").into()),
        }
    }
    println!("ash Vulkan triangle: executing plan and waiting for completion");
    device.execute_plan_with_resources(&plan, &mut resources)?;
    let pixels = device.readback_rgba8(module.textures[0].id)?;
    let mut checked_pixels = 0usize;
    let diagonal = 2 * u64::from(width) * u64::from(height);
    let edge_margin = 2 * (u64::from(width) + u64::from(height));
    for (index, actual) in pixels.chunks_exact(4).enumerate() {
        let x = index as u64 % u64::from(width);
        let y = index as u64 / u64::from(width);
        // Pixel centers lie inside when (x+.5)/width + (y+.5)/height < 1.
        // Exclude a narrow diagonal band to avoid edge coverage conventions.
        let position = (2 * x + 1) * u64::from(height) + (2 * y + 1) * u64::from(width);
        if position.abs_diff(diagonal) <= edge_margin {
            continue;
        }
        let expected = if position < diagonal {
            [51u8, 204, 255, 255]
        } else {
            [0, 0, 0, 255]
        };
        if actual
            .iter()
            .zip(expected)
            .any(|(&actual, expected)| actual.abs_diff(expected) > 1)
        {
            return Err(format!(
                "triangle pixel ({x},{y}) mismatch: {actual:?}, expected {expected:?}"
            )
            .into());
        }
        checked_pixels += 1;
    }
    if checked_pixels == 0 || device.readback_rgba8(module.textures[0].id)? != pixels {
        return Err("triangle repeat readback mismatch or no checked pixels".into());
    }
    println!(
        "ash Vulkan dynamic-rendering triangle OK: {}x{}, {} RGBA8 pixels checked, repeated readback identical",
        width, height, checked_pixels
    );
    Ok(())
}

fn maxwell_shader_inspect(path: PathBuf, stage: &str) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_ir::{ShaderStage, maxwell_shader};
    let bytes = fs::read(path)?;
    let stage = match stage {
        "vertex" => ShaderStage::Vertex,
        "tess-control" => ShaderStage::TessControl,
        "tess-eval" => ShaderStage::TessEvaluation,
        "geometry" => ShaderStage::Geometry,
        "fragment" => ShaderStage::Fragment,
        "compute" => ShaderStage::Compute,
        other => return Err(format!("unknown shader stage {other}").into()),
    };
    let program = if stage == ShaderStage::Compute {
        maxwell_shader::parse_compute(&bytes)?
    } else {
        maxwell_shader::parse_graphics(stage, &bytes)?
    };
    let instructions: usize = program
        .bundles
        .iter()
        .map(|bundle| bundle.instructions.len())
        .sum();
    println!(
        "Maxwell shader: stage={:?} bundles={} instructions={} header={}",
        program.stage,
        program.bundles.len(),
        instructions,
        program.header.is_some()
    );
    Ok(())
}

fn save_journal_smoke(directory: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_vfs::{
        GuestPath, PersistentSaveJournal, SaveTransaction, VirtualFileSystem,
        backend::host::HostDirectoryFileSystem,
    };
    use std::sync::Arc;
    fs::create_dir_all(&directory)?;
    let backend: Arc<dyn VirtualFileSystem> =
        Arc::new(HostDirectoryFileSystem::new(&directory, false)?);
    let journal = PersistentSaveJournal::new(directory.join(".exosphere-save.journal"));
    journal.recover(backend.clone())?;
    let path = GuestPath::parse("/exosphere-smoke-save.bin")?;
    if backend.stat(&path).is_ok() {
        backend.delete_file(&path)?;
    }
    let mut transaction = SaveTransaction::persistent(backend.clone(), journal);
    transaction.create_file(path.clone(), 16);
    transaction.write(path.clone(), 0, b"EXOSPHERE-SAVE!!".to_vec());
    transaction.commit()?;
    let mut bytes = [0u8; 16];
    backend.read_file(&path, 0, &mut bytes)?;
    println!(
        "persistent save journal smoke OK: {}",
        String::from_utf8_lossy(&bytes)
    );
    Ok(())
}

fn pfifo_inspect(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % 4 != 0 {
        return Err("PFIFO input size must be a multiple of 4 bytes".into());
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4-byte word")))
        .collect();
    let writes = exosphere_gpu_ir::decode_pushbuffer(&words)?;
    let mut tracker = exosphere_gpu_ir::SubchannelTracker::default();
    println!(
        "PFIFO: {} words -> {} method writes",
        words.len(),
        writes.len()
    );
    for write in writes {
        if let Some(routed) = tracker.route(write) {
            println!(
                "subch={} class={:?} method=0x{:03x} arg=0x{:08x}",
                routed.subchannel, routed.class, routed.method, routed.argument
            );
        } else {
            println!(
                "subch={} SetObject class={:?}",
                write.subchannel,
                tracker.class_for(write.subchannel)
            );
        }
    }
    Ok(())
}

fn maxwell_frontend_inspect(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() % 4 != 0 {
        return Err("PFIFO input size must be a multiple of 4 bytes".into());
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("4-byte word")))
        .collect();
    let mut frontend = exosphere_gpu_ir::MaxwellFrontEnd::new();
    let trace = frontend.process_pushbuffer(&words)?;
    println!(
        "Maxwell frontend: words={} methods={} routed={} graphics={} compute={} dma={} inline={} 2d={} opaque-engine={}",
        words.len(),
        trace.method_writes,
        trace.routed_writes,
        trace.graphics_events.len(),
        trace.compute_events.len(),
        trace.dma_events.len(),
        trace.inline_events.len(),
        trace.twod_events.len(),
        trace.opaque_engine_writes.len()
    );
    println!(
        "program-region raw: A=0x{:08x} B=0x{:08x}; MME instructions={} starts={}",
        trace.graphics_state.program_region.a,
        trace.graphics_state.program_region.b,
        trace.graphics_state.mme.instruction_ram.len(),
        trace.graphics_state.mme.start_addresses.len()
    );
    if let Some(macro_call) = &trace.graphics_state.active_macro {
        println!(
            "active macro {} params={:?}",
            macro_call.index, macro_call.parameters
        );
    }
    if !trace.dma_events.is_empty() {
        println!("DMA state: {:?}", trace.dma_state);
        for event in trace.dma_events.iter().take(16) {
            println!("  DMA {event:?}");
        }
    }
    for event in trace.graphics_events.iter().take(64) {
        println!("  {event:?}");
    }
    if trace.graphics_events.len() > 64 {
        println!("  ... {} more events", trace.graphics_events.len() - 64);
    }
    Ok(())
}

fn gpu_address_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_ir::{GpuAddressSpace, GpuMapPermissions, GpuMapping, ResourceId};
    let mut space = GpuAddressSpace::new();
    space.map(GpuMapping {
        guest_base: 0x1000,
        size: 0x1000,
        resource: ResourceId(42),
        resource_offset: 0x80,
        permissions: GpuMapPermissions::READ_WRITE,
    })?;
    let resolved = space.resolve(0x1180, 0x20)?;
    println!(
        "GPU address smoke OK: guest=0x1180 -> resource={} offset=0x{:x} len=0x{:x}",
        resolved.resource.0, resolved.offset, resolved.length
    );
    Ok(())
}

fn texture_footprint(
    width: u32,
    height: u32,
    format: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_ir::{TextureFormat, linear_footprint};
    let format = match format {
        "rgba8" => TextureFormat::Rgba8Unorm,
        "bgra8" => TextureFormat::Bgra8Unorm,
        "bc1" => TextureFormat::Bc1RgbaUnorm,
        "bc3" => TextureFormat::Bc3RgbaUnorm,
        "bc5" => TextureFormat::Bc5Unorm,
        "astc4x4" => TextureFormat::Astc4x4Unorm,
        other => return Err(format!("unknown format {other}").into()),
    };
    let fp =
        linear_footprint(format, width, height, 1, 1).ok_or("format has no known footprint")?;
    println!(
        "texture footprint: {:?} {}x{} -> blocks={}x{} row={} bytes total={} bytes",
        format, width, height, fp.blocks_x, fp.blocks_y, fp.row_bytes, fp.total_bytes
    );
    Ok(())
}

fn block_linear_smoke(
    width_bytes: u32,
    height_rows: u32,
    block_height: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    let layout =
        exosphere_gpu_ir::BlockLinearLayout::new(width_bytes, height_rows, 1, block_height, 0);
    let probes = [
        (0, 0),
        (16, 0),
        (32, 0),
        (0, 1),
        (0, 8),
        (width_bytes.saturating_sub(1), height_rows.saturating_sub(1)),
    ];
    println!(
        "block-linear: {}x{} bytes/rows block-height-log2={} storage={} bytes",
        width_bytes,
        height_rows,
        block_height,
        layout.storage_size()
    );
    for (x, y) in probes {
        println!("  ({x},{y}) -> {:?}", layout.offset(x, y, 0));
    }
    Ok(())
}

fn descriptor_inspect(path: PathBuf, kind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() < 32 {
        return Err("descriptor input must contain at least 0x20 bytes".into());
    }
    let mut words = [0u32; 8];
    for (index, chunk) in bytes[..32].chunks_exact(4).enumerate() {
        words[index] = u32::from_le_bytes(chunk.try_into().expect("word"));
    }
    match kind {
        "tic" => {
            let tic = exosphere_gpu_ir::decode_tic(words, None);
            println!(
                "TIC: address=0x{:016x} format={:?} layout={:?} size={:?}x{:?}x{:?} mips={:?}",
                tic.address.high_low(),
                tic.format,
                tic.layout,
                tic.width,
                tic.height,
                tic.depth,
                tic.mip_levels
            );
        }
        "tsc" => {
            let (_, state) = exosphere_gpu_ir::decode_tsc(words, None);
            println!(
                "TSC: address={:?}/{:?}/{:?} filters min={:?} mag={:?} mip={:?} aniso-raw={}",
                state.address_u,
                state.address_v,
                state.address_w,
                state.min_filter,
                state.mag_filter,
                state.mip_filter,
                state.anisotropy_raw
            );
        }
        other => return Err(format!("descriptor kind must be tic or tsc, got {other}").into()),
    }
    Ok(())
}

fn mme_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_ir::mme_exec::{MmeInstruction, execute_decoded};
    let program = [
        MmeInstruction::LoadParameter {
            dst: 1,
            exit: false,
        },
        MmeInstruction::LoadParameter {
            dst: 2,
            exit: false,
        },
        MmeInstruction::SetMethodAddress {
            src: 1,
            exit: false,
        },
        MmeInstruction::Send {
            src: 2,
            exit: false,
        },
        MmeInstruction::Exit,
    ];
    let out = execute_decoded(&program, &[0x585, 0x1234], 32);
    println!(
        "MME semantic smoke: steps={} params={} emissions={:?} stop={:?}",
        out.steps, out.parameters_consumed, out.emissions, out.stop_reason
    );
    Ok(())
}

fn gpu_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_vulkan::device::{RawLinuxVulkanDevice, VulkanDevice};
    match RawLinuxVulkanDevice::create() {
        Ok(device) => println!(
            "{} capabilities: {:?}",
            device.name(),
            device.capabilities()
        ),
        Err(error) => println!("Vulkan device unavailable: {error}"),
    }
    Ok(())
}

fn envydis_oracle(input: PathBuf, program: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(input)?;
    let lines = exosphere_gpu_ir::envydis_oracle::disassemble_gm107(&program, &bytes)
        .map_err(|e| format!("envydis oracle failed: {e}"))?;
    println!("envydis gm107 oracle: {} lines", lines.len());
    for line in lines.iter().take(256) {
        println!("{}", line.raw);
    }
    if lines.len() > 256 {
        println!("... {} more lines", lines.len() - 256);
    }
    Ok(())
}

fn gpu_transfer_smoke() -> Result<(), Box<dyn std::error::Error>> {
    use exosphere_gpu_ir::{
        BufferDesc, BufferUsage, Command as GpuCommand, GpuModule, InMemoryGpuResources, ResourceId,
    };

    let source = ResourceId(0x100);
    let destination = ResourceId(0x200);
    let mut module = GpuModule::default();
    module.buffers.push(BufferDesc {
        id: source,
        size: 8,
        usage: BufferUsage::TransferSource,
    });
    module.buffers.push(BufferDesc {
        id: destination,
        size: 8,
        usage: BufferUsage::TransferDestination,
    });
    module.commands.push(GpuCommand::Copy {
        source,
        destination,
        source_offset: 0,
        destination_offset: 0,
        bytes: 8,
    });
    module.commands.push(GpuCommand::Fill {
        destination,
        value: 0xAABB_CCDD,
        bytes: 4,
    });

    let device = exosphere_gpu_vulkan::device::RawLinuxVulkanDevice::create()?;
    let mut backend = exosphere_gpu_vulkan::VulkanBackend::with_device(Box::new(device));
    let mut resources = InMemoryGpuResources::new();
    resources.insert(source, vec![1, 2, 3, 4, 5, 6, 7, 8], false);
    resources.insert(destination, vec![0; 8], true);

    backend.execute_with_resources(&module, &mut resources)?;
    let result = resources
        .bytes(destination)
        .ok_or("destination payload disappeared")?;
    println!("GPU native Vulkan transfer smoke OK: {:02x?}", result);
    Ok(())
}
