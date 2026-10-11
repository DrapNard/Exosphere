// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercise the runtime bridge through native HIPC/CMIF driver requests.

use super::queue::{ChannelFault, ChannelStep, GuestGpuChannel};
use exosphere_horizon::{
    HorizonProfileId, cmif,
    handle::Handle,
    hipc,
    kernel::HorizonKernel,
    memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
};

struct NativeDriver {
    kernel: HorizonKernel,
    process: u64,
    thread: u64,
    session: Handle,
    tls: u64,
}

impl NativeDriver {
    fn new() -> Self {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let process = kernel.create_process(1, "native GPU bridge test");
        let (thread, _) = kernel.create_thread(process, 0x1000, 0x9000, 44).unwrap();
        for (address, size) in [
            (0x4000, PAGE_SIZE),
            (0x8000, 3 * PAGE_SIZE),
            (0xb000, 2 * PAGE_SIZE),
        ] {
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
        request[8..12].copy_from_slice(&4u32.to_le_bytes()); // Two copied handles.
        request[12..16].copy_from_slice(&0xffff8001u32.to_le_bytes());
        request[16..20].copy_from_slice(&transfer.0.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&3u32.to_le_bytes());
        request[48..52].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        assert_eq!(&driver.invoke(request)[..4], &0u32.to_le_bytes());
        driver.kernel.close_handle(process, transfer).unwrap();
        driver
    }

    fn write(&mut self, address: u64, bytes: &[u8]) {
        self.kernel
            .process_mut(self.process)
            .unwrap()
            .memory
            .write(address, bytes)
            .unwrap();
    }

    fn read(&self, address: u64, size: usize) -> Vec<u8> {
        let mut bytes = vec![0; size];
        self.kernel
            .process(self.process)
            .unwrap()
            .memory
            .read(address, &mut bytes)
            .unwrap();
        bytes
    }

    fn invoke(&mut self, request: [u8; 0x100]) -> Vec<u8> {
        self.write(self.tls, &request);
        self.kernel
            .send_guest_sync_request(self.process, self.thread, self.session)
            .unwrap();
        let response = self.read(self.tls, 0x100);
        let message = hipc::parse(&response).unwrap();
        let response = cmif::parse_response(message.raw_data).unwrap();
        assert_eq!(response.header.result, 0);
        response.payload.to_vec()
    }

    fn open(&mut self, path: &[u8]) -> u32 {
        self.write(0x8000, path);
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

    fn close(&mut self, descriptor: u32) {
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&9u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&2u32.to_le_bytes());
        request[32..36].copy_from_slice(&descriptor.to_le_bytes());
        assert_eq!(&self.invoke(request)[..4], &0u32.to_le_bytes());
    }

    fn ioctl(&mut self, fd: u32, code: u32, data: &[u8], inout: bool) -> Vec<u8> {
        self.ioctl_with_status(fd, code, data, inout, 0)
    }

    fn ioctl_with_status(
        &mut self,
        fd: u32,
        code: u32,
        data: &[u8],
        inout: bool,
        status: u32,
    ) -> Vec<u8> {
        self.write(0x9000, data);
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
        request[48..52].copy_from_slice(&fd.to_le_bytes());
        request[52..56].copy_from_slice(&code.to_le_bytes());
        assert_eq!(&self.invoke(request)[..4], &status.to_le_bytes());
        if inout {
            self.read(0xa000, data.len())
        } else {
            Vec::new()
        }
    }

    fn kickoff(&mut self, fd: u32, entry: u64) -> Vec<u8> {
        let mut header = [0; 24];
        header[8..12].copy_from_slice(&1u32.to_le_bytes());
        header[12..16].copy_from_slice(&4u32.to_le_bytes());
        self.write(0x9000, &header);
        self.write(0x9800, &entry.to_le_bytes());
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&(4u32 | (2 << 20) | (1 << 24)).to_le_bytes());
        request[4..8].copy_from_slice(&10u32.to_le_bytes());
        // Ioctl2 has a header send buffer, a separate entry send buffer and
        // header writeback. Raw CMIF starts after all three descriptors.
        for (offset, size, address) in [(8, 24u32, 0x9000u32), (20, 8, 0x9800), (32, 24, 0xa000)] {
            request[offset..offset + 4].copy_from_slice(&size.to_le_bytes());
            request[offset + 4..offset + 8].copy_from_slice(&address.to_le_bytes());
        }
        request[48..52].copy_from_slice(b"SFCI");
        request[56..60].copy_from_slice(&11u32.to_le_bytes());
        request[64..68].copy_from_slice(&fd.to_le_bytes());
        request[68..72].copy_from_slice(&0xc018481bu32.to_le_bytes());
        assert_eq!(&self.invoke(request)[..4], &0u32.to_le_bytes());
        self.read(0xa000, 24)
    }
}

fn verify_gpu_poll_fault_preserves_cpu_progress(driver: &mut NativeDriver) {
    use exosphere_horizon::thread::ThreadState;
    let saved_context = driver.kernel.thread(driver.thread).unwrap().context.clone();
    // ADD X0, X0, #1; STR W0, [X1]; B back to ADD. The mock GPU backend
    // faults only after this guest loop has published a nonzero memory marker.
    let words = [0x91000400u32, 0xb9000020, 0x17fffffe];
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .map(
            0xd000,
            PAGE_SIZE,
            MemoryPermission::READ_WRITE,
            MemoryKind::Heap,
        )
        .unwrap();
    driver.write(
        0xd000,
        &words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .protect(0xd000, PAGE_SIZE, MemoryPermission::READ_EXECUTE)
        .unwrap();
    let thread = driver.kernel.thread_mut(driver.thread).unwrap();
    thread.state = ThreadState::Running;
    thread.context.pc = 0xd000;
    thread.context.x[0] = 0;
    thread.context.x[1] = 0xc900;
    let (completed, stop) =
        crate::execution::run_thread(&mut driver.kernel, driver.process, driver.thread, 8192)
            .unwrap();
    assert_eq!(completed, 4096);
    assert!(matches!(
        stop,
        Some(crate::interpreter::CpuError::Memory(_))
    ));
    let thread = driver.kernel.thread(driver.thread).unwrap();
    assert_eq!(thread.context.pc, 0xd004);
    assert_eq!(thread.context.x[0], 1366);
    assert_eq!(driver.read(0xc900, 4), 1365u32.to_le_bytes());

    // A service fault before scheduling must not consume or strand a ready
    // CPU thread, nor report idle/normal completion to the caller.
    driver.kernel.thread_mut(driver.thread).unwrap().state = ThreadState::Ready;
    driver
        .kernel
        .scheduler
        .enqueue_with_priority(driver.thread, 44);
    let process = crate::execution::run_process(&mut driver.kernel, driver.process, 1).unwrap();
    assert_eq!(process.completed, 0);
    assert!(process.last_thread.is_none());
    assert!(!process.idle);
    assert!(matches!(
        process.stop,
        Some(crate::interpreter::CpuError::Memory(_))
    ));
    assert_eq!(
        driver.kernel.thread(driver.thread).unwrap().state,
        ThreadState::Ready
    );
    assert!(!driver.kernel.scheduler.is_empty());
    driver.kernel.scheduler.remove(driver.thread);
    driver.write(0xc900, &[0; 4]);
    driver.kernel.thread_mut(driver.thread).unwrap().context = saved_context;
    driver
        .kernel
        .process_mut(driver.process)
        .unwrap()
        .memory
        .unmap(0xd000, PAGE_SIZE)
        .unwrap();
}

#[test]
fn native_gpu_teardown_preserves_aliases_and_releases_process_channels() {
    #[derive(Debug)]
    struct ClosingProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl exosphere_horizon::nvdrv::backend::NvGpuIoctlBackend for ClosingProbe {
        fn close_channel(
            &mut self,
            _identity: exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        ) -> exosphere_horizon::HorizonResult<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        fn dispatch(
            &mut self,
            _identity: exosphere_horizon::nvdrv::NvGpuChannelIdentity,
            _memory: &mut exosphere_horizon::memory::AddressSpace,
            _gpu: &exosphere_horizon::nvdrv::address_space::NvGpuAddressSpace,
            _request: &exosphere_horizon::ipc::ServiceRequest,
        ) -> exosphere_horizon::HorizonResult<exosphere_horizon::ipc::ServiceResponse> {
            panic!("teardown test must not dispatch GPU commands")
        }
    }
    for process_exit in [false, true] {
        let mut driver = NativeDriver::new();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        driver
            .kernel
            .install_gpu_ioctl_backend(Box::new(ClosingProbe(calls.clone())))
            .unwrap();
        driver.open(b"/dev/nvhost-gpu");
        let mut control = [0; 0x100];
        control[..4].copy_from_slice(&5u32.to_le_bytes());
        control[4..8].copy_from_slice(&8u32.to_le_bytes());
        control[16..20].copy_from_slice(b"SFCI");
        control[24..28].copy_from_slice(&2u32.to_le_bytes());
        driver.write(driver.tls, &control);
        driver
            .kernel
            .send_guest_sync_request(driver.process, driver.thread, driver.session)
            .unwrap();
        let response = driver.read(driver.tls, 0x100);
        let alias = Handle(hipc::parse(&response).unwrap().handles.move_handles[0]);
        if process_exit {
            driver.kernel.exit_process(driver.process).unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
            driver.kernel.exit_process(driver.process).unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        } else {
            driver
                .kernel
                .close_handle(driver.process, driver.session)
                .unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0);
            driver.kernel.close_handle(driver.process, alias).unwrap();
            assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        }
    }
}

#[test]
fn native_driver_mapping_feeds_bound_channel_execution_without_snapshots() {
    use exosphere_gpu_ir::dma::method;
    let mut driver = NativeDriver::new();
    let map = driver.open(b"/dev/nvmap");
    let space = driver.open(b"/dev/nvhost-as-gpu");
    let channel = driver.open(b"/dev/nvhost-gpu");
    driver.ioctl(space, 0x40284109, &[0; 40], false);
    driver.ioctl(space, 0x40044101, &channel.to_le_bytes(), false);
    driver.ioctl(channel, 0x40044801, &map.to_le_bytes(), false);
    let created = driver.ioctl(
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
    driver.ioctl(map, 0xc0200104, &allocation, true);
    let mut reservation = [0; 24];
    reservation[..4].copy_from_slice(&2u32.to_le_bytes());
    reservation[4..8].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    reservation[8..12].copy_from_slice(&1u32.to_le_bytes());
    reservation[16..24].copy_from_slice(&0x100000u64.to_le_bytes());
    driver.ioctl(space, 0xc0184102, &reservation, true);
    let mut mapping = [0; 40];
    mapping[..4].copy_from_slice(&1u32.to_le_bytes());
    mapping[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    mapping[8..12].copy_from_slice(&handle.to_le_bytes());
    mapping[12..16].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
    mapping[24..32].copy_from_slice(&(2 * PAGE_SIZE).to_le_bytes());
    mapping[32..40].copy_from_slice(&0x100000u64.to_le_bytes());
    driver.ioctl(space, 0xc0284106, &mapping, true);

    let mut words = vec![0x60010000, 0xb0b5];
    for (register, value) in [
        (method::OFFSET_IN_LOWER, 0x100800),
        (method::OFFSET_OUT_LOWER, 0x101100),
        (method::LINE_LENGTH_IN, 8),
        (method::LAUNCH_DMA, 0x182),
    ] {
        words.extend([0x60010000 | u32::from(register), value]);
    }
    words.extend([0x6001001e, 0]); // FIFO WFI after the actual DMA transfer.
    driver.write(
        0xb000,
        &words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect::<Vec<_>>(),
    );
    driver.write(0xb800, &[1, 2, 3, 4, 5, 6, 7, 8]);
    let entry = 0x100000 | ((words.len() as u64) << 42);
    let mut queue = GuestGpuChannel::new(2).unwrap();
    let mut submission_bytes = [0; 32];
    submission_bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    submission_bytes[24..32].copy_from_slice(&entry.to_le_bytes());
    for (offset, value) in [(12, 1u32), (16, 7), (20, 9)] {
        let mut unsupported = submission_bytes;
        unsupported[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let submission =
            exosphere_horizon::nvdrv::submission::GpfifoSubmission::inline(&unsupported, 2)
                .unwrap();
        assert!(
            queue
                .enqueue_submission_bound(
                    &mut driver.kernel,
                    driver.process,
                    driver.session,
                    channel,
                    &submission,
                )
                .is_err()
        );
        assert_eq!(queue.pending_entries(), 0);
        assert!(queue.fault().is_none());
    }
    let request = exosphere_horizon::ipc::ServiceRequest {
        service: "nvdrv".into(),
        command_id: 1,
        values: vec![],
        raw_payload: [channel.to_le_bytes(), 0xc0204808u32.to_le_bytes()].concat(),
        buffers: vec![
            exosphere_horizon::ipc::IpcBuffer {
                address: 0,
                capacity: 32,
                direction: exosphere_horizon::ipc::IpcBufferDirection::In,
                data: submission_bytes.to_vec(),
            },
            exosphere_horizon::ipc::IpcBuffer {
                address: 0,
                capacity: 32,
                direction: exosphere_horizon::ipc::IpcBufferDirection::Out,
                data: vec![],
            },
        ],
    };
    let mut malformed = request.clone();
    malformed.raw_payload[4..8].copy_from_slice(&0xc0284808u32.to_le_bytes());
    malformed.buffers[0].data[8..12].copy_from_slice(&2u32.to_le_bytes());
    malformed.buffers[0]
        .data
        .extend_from_slice(&(1u64 << 32).to_le_bytes());
    malformed.buffers[0].capacity = 40;
    malformed.buffers[1].capacity = 40;
    assert!(
        queue
            .enqueue_request_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                &malformed,
            )
            .is_err()
    );
    assert_eq!(queue.pending_entries(), 0);
    assert!(queue.fault().is_none());
    assert!(malformed.buffers[1].data.is_empty());
    assert_eq!(driver.read(0xc100, 8), [0; 8]);
    let resource = exosphere_gpu_ir::ResourceId(91);
    let mut resources = exosphere_gpu_ir::GpuAddressSpace::new();
    resources
        .map(exosphere_gpu_ir::GpuMapping {
            guest_base: 0x101100,
            size: 8,
            resource,
            resource_offset: 0,
            permissions: exosphere_gpu_ir::GpuMapPermissions::READ_WRITE,
        })
        .unwrap();
    let before = super::shader_memory::with_channel_resources(
        &mut driver.kernel,
        driver.process,
        driver.session,
        channel,
        &resources,
        |provider| provider.content_fingerprint(resource, 0, 8),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        queue
            .enqueue_request_bound(&mut driver.kernel, driver.process, driver.session, &request,)
            .unwrap(),
        1
    );
    // Admission captures descriptors only; execution is a distinct operation.
    assert_eq!(driver.read(0xc100, 8), [0; 8]);
    assert_eq!(
        queue
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .unwrap(),
        ChannelStep::Applied { transfers: 1 }
    );
    assert_eq!(driver.read(0xc100, 8), [1, 2, 3, 4, 5, 6, 7, 8]);
    super::shader_memory::with_channel_resources(
        &mut driver.kernel,
        driver.process,
        driver.session,
        channel,
        &resources,
        |provider| {
            assert_ne!(
                provider.content_fingerprint(resource, 0, 8).unwrap(),
                before
            );
            let mut bytes = [0; 8];
            provider.read(resource, 0, &mut bytes).unwrap();
            assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        },
    )
    .unwrap();

    let second_channel = driver.open(b"/dev/nvhost-gpu");
    #[derive(Debug)]
    struct QueueBackendProbe {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        polls:
            std::sync::Arc<std::sync::Mutex<Vec<exosphere_horizon::nvdrv::NvGpuChannelIdentity>>>,
        backend: super::nv_backend::NativeGpuBackend,
    }
    impl exosphere_horizon::nvdrv::backend::NvGpuIoctlBackend for QueueBackendProbe {
        fn poll_channel(
            &mut self,
            identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity,
            memory: &mut exosphere_horizon::memory::AddressSpace,
            gpu: &exosphere_horizon::nvdrv::address_space::NvGpuAddressSpace,
            maximum_entries: usize,
        ) -> exosphere_horizon::HorizonResult<usize> {
            assert_eq!(maximum_entries, 1);
            self.polls.lock().unwrap().push(identity.clone());
            let mut fault_marker = [0; 4];
            memory.read(0xc900, &mut fault_marker).unwrap();
            if fault_marker != [0; 4] {
                return Err(exosphere_horizon::HorizonError::InvalidState(
                    "injected GPU polling fault after CPU memory effects".into(),
                ));
            }
            exosphere_horizon::nvdrv::backend::NvGpuIoctlBackend::poll_channel(
                &mut self.backend,
                identity,
                memory,
                gpu,
                maximum_entries,
            )
        }
        fn close_channel(
            &mut self,
            identity: exosphere_horizon::nvdrv::NvGpuChannelIdentity,
        ) -> exosphere_horizon::HorizonResult<()> {
            assert_eq!(self.backend.pending_entries(&identity), Some(0));
            self.backend.close_channel(identity.clone())?;
            assert_eq!(self.backend.pending_entries(&identity), None);
            self.calls
                .fetch_add(100, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        fn dispatch(
            &mut self,
            identity: exosphere_horizon::nvdrv::NvGpuChannelIdentity,
            memory: &mut exosphere_horizon::memory::AddressSpace,
            gpu: &exosphere_horizon::nvdrv::address_space::NvGpuAddressSpace,
            request: &exosphere_horizon::ipc::ServiceRequest,
        ) -> exosphere_horizon::HorizonResult<exosphere_horizon::ipc::ServiceResponse> {
            assert!(matches!(request.command_id, 1 | 11));
            let mut bytes = [0; 8];
            gpu.read(memory, 0x100800, &mut bytes).unwrap();
            assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let ioctl = u32::from_le_bytes(request.raw_payload[4..8].try_into().unwrap());
            if ioctl == 0xc0104809 {
                for (offset, invalid_word) in [(0, 0xb1c0u32), (4, 1)] {
                    let mut invalid = request.clone();
                    invalid.buffers[0].data[offset..offset + 4]
                        .copy_from_slice(&invalid_word.to_le_bytes());
                    assert!(
                        self.backend
                            .dispatch(identity.clone(), memory, gpu, &invalid)
                            .is_err()
                    );
                }
                let response = self
                    .backend
                    .dispatch(identity.clone(), memory, gpu, request)?;
                assert!(
                    self.backend
                        .dispatch(identity, memory, gpu, request)
                        .is_err()
                );
                return Ok(response);
            }
            if ioctl & 0xc000ffff == 0xc0004808 || ioctl == 0xc018481b {
                let mut unsupported = request.clone();
                unsupported.buffers[0].data[12..16].copy_from_slice(&0x104u32.to_le_bytes());
                assert!(
                    self.backend
                        .dispatch(identity.clone(), memory, gpu, &unsupported)
                        .is_err()
                );
                assert_eq!(self.backend.pending_entries(&identity), Some(0));
                let response = self
                    .backend
                    .dispatch(identity.clone(), memory, gpu, request)?;
                assert_eq!(self.backend.pending_entries(&identity), Some(1));
                let mut destination = [0; 8];
                gpu.read(memory, 0x101100, &mut destination).unwrap();
                assert_eq!(destination, [0; 8]); // Admission did not execute DMA.
                return Ok(response);
            }
            if request
                .buffers
                .iter()
                .any(|buffer| buffer.data.first() == Some(&2))
            {
                return self.backend.dispatch(identity, memory, gpu, request);
            }
            // This test-only rejection branch does not allocate a queue or
            // return a synthetic fence/success response.
            let mut response = exosphere_horizon::ipc::ServiceResponse::success();
            response.raw_payload = 4u32.to_le_bytes().to_vec();
            Ok(response)
        }
    }
    let backend_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend_polls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    #[derive(Debug)]
    struct DrawProbe {
        identities:
            std::sync::Arc<std::sync::Mutex<Vec<exosphere_horizon::nvdrv::NvGpuChannelIdentity>>>,
        closed:
            std::sync::Arc<std::sync::Mutex<Vec<exosphere_horizon::nvdrv::NvGpuChannelIdentity>>>,
    }
    impl super::nv_backend::NativeDrawRenderer for DrawProbe {
        fn close_channel(&mut self, identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity) {
            let mut closed = self.closed.lock().unwrap();
            if !closed.contains(identity) {
                closed.push(identity.clone());
            }
        }
        fn draw(
            &mut self,
            identity: &exosphere_horizon::nvdrv::NvGpuChannelIdentity,
            memory: &mut exosphere_horizon::memory::AddressSpace,
            gpu: &exosphere_horizon::nvdrv::address_space::NvGpuAddressSpace,
            event: &exosphere_gpu_ir::MaxwellEvent,
        ) -> Result<(), super::GuestGpuExecutionFailure> {
            assert!(matches!(
                event,
                exosphere_gpu_ir::MaxwellEvent::DrawVertexArray {
                    first: 4,
                    count: 3,
                    ..
                }
            ));
            self.identities.lock().unwrap().push(identity.clone());
            // A test marker proves live canonical memory reaches the installed
            // renderer; it is not a rendered pixel or substitute game output.
            gpu.write(memory, 0x101300, &17u32.to_le_bytes()).unwrap();
            Ok(())
        }
    }
    let draw_identities = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let renderer_closed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    driver
        .kernel
        .install_gpu_ioctl_backend(Box::new(QueueBackendProbe {
            calls: backend_calls.clone(),
            polls: backend_polls.clone(),
            backend: super::nv_backend::NativeGpuBackend::with_renderer(
                1,
                Box::new(DrawProbe {
                    identities: draw_identities.clone(),
                    closed: renderer_closed.clone(),
                }),
            )
            .unwrap(),
        }))
        .unwrap();
    assert!(
        driver
            .kernel
            .install_gpu_ioctl_backend(Box::new(QueueBackendProbe {
                calls: backend_calls.clone(),
                polls: backend_polls.clone(),
                backend: super::nv_backend::NativeGpuBackend::new(1).unwrap(),
            }))
            .is_err()
    );
    driver.ioctl_with_status(channel, 0xc020481a, &[0; 32], false, 4);
    assert_eq!(backend_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    let mut native_allocation = [0; 32];
    native_allocation[..4].copy_from_slice(&2u32.to_le_bytes());
    native_allocation[4..8].copy_from_slice(&1u32.to_le_bytes());
    native_allocation[12..20].fill(0xff); // Output inputs must not manufacture counters.
    let allocated = driver.ioctl(channel, 0xc020481a, &native_allocation, true);
    assert_ne!(u32::from_le_bytes(allocated[12..16].try_into().unwrap()), 0);
    assert_eq!(&allocated[16..20], &0u32.to_le_bytes());
    assert_eq!(backend_calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    let mut object_request = [0; 16];
    object_request[..4].copy_from_slice(&0xb197u32.to_le_bytes());
    object_request[8..16].fill(0xff);
    let object = driver.ioctl(channel, 0xc0104809, &object_request, true);
    let object_id = u64::from_le_bytes(object[8..16].try_into().unwrap());
    assert_ne!(object_id, 0);
    assert_ne!(object_id, u64::MAX);
    assert_eq!(object_id, 1); // Rejected class/flag requests consumed no identity.
    driver.write(0xc100, &[0; 8]);
    let mut native_submission = [0; 32];
    native_submission[8..12].copy_from_slice(&1u32.to_le_bytes());
    native_submission[12..16].copy_from_slice(&4u32.to_le_bytes());
    native_submission[24..32].copy_from_slice(&entry.to_le_bytes());
    driver.ioctl(channel, 0xc0204808, &native_submission, true);
    assert_eq!(driver.read(0xc100, 8), [0; 8]);
    // A CPU thread waiting on services must not stop independent GPU work.
    driver.kernel.thread_mut(driver.thread).unwrap().state =
        exosphere_horizon::thread::ThreadState::Waiting;
    let (executed, stop) =
        crate::execution::run_thread(&mut driver.kernel, driver.process, driver.thread, 1).unwrap();
    assert_eq!(executed, 0);
    assert!(stop.is_none());
    driver.kernel.thread_mut(driver.thread).unwrap().state =
        exosphere_horizon::thread::ThreadState::Ready;
    assert_eq!(driver.read(0xc100, 8), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(backend_calls.load(std::sync::atomic::Ordering::Relaxed), 4);
    driver.write(0xc100, &[0; 8]);
    let kickoff_reply = driver.kickoff(channel, entry);
    assert_eq!(&kickoff_reply[16..24], &[0; 8]); // No completion fence was requested.
    assert_eq!(driver.read(0xc100, 8), [0; 8]);
    // Read live source memory at execution, not an admission-time snapshot.
    driver.write(0xb800, &[8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(
        driver.kernel.poll_gpu_process(driver.process, 64).unwrap(),
        1
    );
    assert_eq!(driver.read(0xc100, 8), [8, 7, 6, 5, 4, 3, 2, 1]);
    driver.write(0xb800, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(backend_calls.load(std::sync::atomic::Ordering::Relaxed), 5);
    let owner = driver
        .kernel
        .with_gpu_channel(driver.process, driver.session, channel, |identity, _, _| {
            identity
        })
        .unwrap();
    let mut points = exosphere_horizon::nvdrv::syncpoint::NvGpuSyncpoints::new(1).unwrap();
    let initial = points.allocate(owner.clone()).unwrap();
    let mut synchronized = GuestGpuChannel::new(2).unwrap();
    assert!(
        synchronized
            .enqueue_synchronized_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &[0, 0, 0],
                super::queue::SyncpointAdmission {
                    points: &mut points,
                    id: initial.id,
                    increments: 7
                },
            )
            .is_err()
    );
    assert_eq!(synchronized.pending_entries(), 0);
    let fence = synchronized
        .enqueue_synchronized_bound(
            &mut driver.kernel,
            driver.process,
            driver.session,
            channel,
            &[entry, 0],
            super::queue::SyncpointAdmission {
                points: &mut points,
                id: initial.id,
                increments: 3,
            },
        )
        .unwrap();
    assert_eq!(fence.value, 3); // Failed admission did not consume seven increments.
    for retired in 0..=2 {
        let completed = synchronized
            .retire_syncpoints_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &mut points,
            )
            .unwrap();
        assert_eq!(completed, usize::from(retired == 2));
        assert_eq!(
            points.completed(&owner, initial.id).unwrap(),
            if retired == 2 { fence } else { initial }
        );
        if retired < 2 {
            assert_eq!(
                synchronized
                    .step_bound(&mut driver.kernel, driver.process, driver.session, channel,)
                    .unwrap(),
                ChannelStep::Applied {
                    transfers: usize::from(retired == 0)
                }
            );
        }
    }
    driver.ioctl(space, 0x40044101, &second_channel.to_le_bytes(), false);
    driver.ioctl(second_channel, 0x40044801, &map.to_le_bytes(), false);
    backend_polls.lock().unwrap().clear();
    for _ in 0..3 {
        assert_eq!(
            driver.kernel.poll_gpu_process(driver.process, 1).unwrap(),
            0
        );
    }
    {
        let polls = backend_polls.lock().unwrap();
        assert_eq!(polls.len(), 3);
        assert_ne!(polls[0], polls[1]);
        assert_eq!(polls[0], polls[2]); // Wrap fairly rather than restarting at fd 1.
    }
    // Exercise constant uploads through native driver admission and the owned
    // channel queue, not just a separately constructed trace executor.
    let constant_methods = exosphere_gpu_ir::maxwell_state::method::CONSTANT_BUFFER_SELECTOR_SIZE;
    let constant_words = [
        0x60010000u32,
        0xb197,
        (1 << 29) | (3 << 16) | u32::from(constant_methods),
        8,
        0,
        0x101200,
        (5 << 29)
            | (3 << 16)
            | u32::from(exosphere_gpu_ir::maxwell_state::method::CONSTANT_BUFFER_UPLOAD_OFFSET),
        0,
        0x80000000,
        0x7fa12345,
    ];
    driver.write(
        0xb600,
        &constant_words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    driver.write(0xc200, &[0; 8]);
    let constant_entry = 0x100600 | ((constant_words.len() as u64) << 42);
    driver.write(0xc100, &[0; 8]); // Existing backend admission guard checks this DMA target.
    driver.kickoff(channel, constant_entry);
    assert_eq!(driver.read(0xc200, 8), [0; 8]); // Admission is not execution.
    assert_eq!(
        driver.kernel.poll_gpu_process(driver.process, 64).unwrap(),
        1
    );
    assert_eq!(
        driver.read(0xc200, 8),
        [0x80000000u32, 0x7fa12345]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
    );
    let draw_words = [
        0x60010000u32,
        0xb197,
        0x60010000 | u32::from(exosphere_gpu_ir::maxwell_state::method::SET_VERTEX_ARRAY_START),
        4,
        0x60010000 | u32::from(exosphere_gpu_ir::maxwell_state::method::DRAW_VERTEX_ARRAY),
        3,
    ];
    driver.write(
        0xb600,
        &draw_words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    driver.write(0xc300, &[0; 4]);
    driver.kickoff(channel, 0x100600 | ((draw_words.len() as u64) << 42));
    assert!(draw_identities.lock().unwrap().is_empty());
    assert_eq!(driver.read(0xc300, 4), [0; 4]);
    assert_eq!(
        driver.kernel.poll_gpu_process(driver.process, 64).unwrap(),
        1
    );
    assert_eq!(driver.read(0xc300, 4), 17u32.to_le_bytes());
    assert_eq!(
        draw_identities.lock().unwrap().as_slice(),
        std::slice::from_ref(&owner)
    );
    assert_eq!(
        driver.kernel.poll_gpu_process(driver.process, 64).unwrap(),
        0
    );
    assert_eq!(draw_identities.lock().unwrap().len(), 1);
    verify_gpu_poll_fault_preserves_cpu_progress(&mut driver);
    // A blocked head must keep its fence pending, including after a durable
    // execution fault. Use real nvmap-backed semaphore memory, not a snapshot.
    let mut wait_points = exosphere_horizon::nvdrv::syncpoint::NvGpuSyncpoints::new(1).unwrap();
    let wait_initial = wait_points.allocate(owner.clone()).unwrap();
    let mut waiting = GuestGpuChannel::new(1).unwrap();
    let wait_words = [0x20040004u32, 0, 0x101800, 7, 1];
    driver.write(
        0xb400,
        &wait_words
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let wait_entry = 0x100400 | (5u64 << 42);
    let wait_fence = waiting
        .enqueue_synchronized_bound(
            &mut driver.kernel,
            driver.process,
            driver.session,
            channel,
            &[wait_entry],
            super::queue::SyncpointAdmission {
                points: &mut wait_points,
                id: wait_initial.id,
                increments: 1,
            },
        )
        .unwrap();
    assert_eq!(
        waiting
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .unwrap(),
        ChannelStep::Waiting { transfers: 0 }
    );
    assert_eq!(
        waiting
            .retire_syncpoints_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &mut wait_points,
            )
            .unwrap(),
        0
    );
    assert_eq!(
        wait_points.completed(&owner, wait_initial.id).unwrap(),
        wait_initial
    );
    driver.write(0xc800, &7u32.to_le_bytes());
    assert_eq!(
        waiting
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .unwrap(),
        ChannelStep::Applied { transfers: 0 }
    );
    assert_eq!(
        waiting
            .retire_syncpoints_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &mut wait_points,
            )
            .unwrap(),
        1
    );
    assert_eq!(
        wait_points.completed(&owner, wait_initial.id).unwrap(),
        wait_fence
    );

    // The same valid descriptor now points its semaphore outside the mapped VA.
    driver.write(0xb408, &0x200000u32.to_le_bytes());
    waiting
        .enqueue_synchronized_bound(
            &mut driver.kernel,
            driver.process,
            driver.session,
            channel,
            &[wait_entry],
            super::queue::SyncpointAdmission {
                points: &mut wait_points,
                id: wait_initial.id,
                increments: 1,
            },
        )
        .unwrap();
    assert_eq!(
        waiting
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .unwrap(),
        ChannelStep::Stopped
    );
    assert_eq!(
        waiting
            .retire_syncpoints_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &mut wait_points,
            )
            .unwrap(),
        0
    );
    assert_eq!(
        wait_points.completed(&owner, wait_initial.id).unwrap(),
        wait_fence
    );
    // Capacity rejection must not bind an otherwise unused queue. A different
    // authenticated channel can subsequently admit and consume its first batch.
    let mut admission_queue = GuestGpuChannel::new(1).unwrap();
    assert!(
        admission_queue
            .enqueue_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                channel,
                &[entry, entry],
            )
            .is_err()
    );
    assert_eq!(admission_queue.pending_entries(), 0);
    assert!(admission_queue.fault().is_none());
    admission_queue
        .enqueue_bound(
            &mut driver.kernel,
            driver.process,
            driver.session,
            second_channel,
            &[entry],
        )
        .unwrap();
    assert!(
        admission_queue
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .is_err()
    );
    assert_eq!(admission_queue.pending_entries(), 1);
    assert_eq!(
        admission_queue
            .step_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                second_channel,
            )
            .unwrap(),
        ChannelStep::Applied { transfers: 1 }
    );
    assert!(
        queue
            .enqueue_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                second_channel,
                &[entry],
            )
            .is_err()
    );
    assert_eq!(queue.pending_entries(), 0);
    assert!(queue.fault().is_none());
    let split_submission = exosphere_horizon::nvdrv::submission::GpfifoSubmission::split(
        &submission_bytes[..24],
        &submission_bytes[24..],
        2,
    )
    .unwrap();
    queue
        .enqueue_submission_bound(
            &mut driver.kernel,
            driver.process,
            driver.session,
            channel,
            &split_submission,
        )
        .unwrap();
    assert!(
        queue
            .step_bound(
                &mut driver.kernel,
                driver.process,
                driver.session,
                second_channel
            )
            .is_err()
    );
    assert_eq!(queue.pending_entries(), 1);
    assert!(queue.fault().is_none());
    // A later native unmap must be observed by the same queue, rather than
    // continuing to execute against a mapping snapshot captured at binding.
    driver.ioctl(space, 0xc0084105, &0x100000u64.to_le_bytes(), true);
    assert_eq!(
        queue
            .step_bound(&mut driver.kernel, driver.process, driver.session, channel)
            .unwrap(),
        ChannelStep::Stopped
    );
    assert!(matches!(queue.fault(), Some(ChannelFault::Decode(_))));
    assert_eq!(driver.read(0xc100, 8), [1, 2, 3, 4, 5, 6, 7, 8]);
    // Closing the native descriptor must revoke backend access before its
    // callback runs, even if the provider's resource mapping remains present.
    driver.close(channel);
    assert!(renderer_closed.lock().unwrap().contains(&owner));
    assert_eq!(
        backend_calls.load(std::sync::atomic::Ordering::Relaxed),
        107
    );
    let mut invoked = false;
    assert!(
        super::shader_memory::with_channel_resources(
            &mut driver.kernel,
            driver.process,
            driver.session,
            channel,
            &resources,
            |_| invoked = true,
        )
        .is_err()
    );
    assert!(!invoked);
    // Closing the first channel releases backend capacity. A live second
    // channel receives fresh synchronization/object IDs, never stale aliases.
    driver.ioctl(space, 0xc0284106, &mapping, true);
    let second_allocation = driver.ioctl(second_channel, 0xc020481a, &native_allocation, true);
    assert_ne!(&second_allocation[12..16], &allocated[12..16]);
    let second_object = driver.ioctl(second_channel, 0xc0104809, &object_request, true);
    let second_object_id = u64::from_le_bytes(second_object[8..16].try_into().unwrap());
    assert!(second_object_id > object_id);
    driver.close(second_channel);
}
