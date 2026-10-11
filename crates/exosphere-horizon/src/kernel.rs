// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Minimal, host-neutral Horizon compatibility kernel.

use crate::{
    HorizonProfileId,
    display::DisplayManager,
    error::{HorizonError, HorizonResult},
    fs::HorizonFs,
    fsp::FspSrvService,
    handle::KernelObject,
    hid::HidState,
    ipc::{ServiceRegistry, SmService},
    process::{HorizonProcess, ProcessState},
    scheduler::Scheduler,
    thread::{HorizonThread, ThreadState},
    time::TimeManager,
};
use std::collections::BTreeMap;

mod condition;
mod mutex;

#[derive(Debug)]
struct SynchronizationWait {
    objects: Vec<KernelObject>,
    deadline_ns: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum SessionTarget {
    Hid(crate::hid_service::HidObject),
    Bsd(crate::bsd::BsdClient),
    Nifm(crate::nifm::NifmObject),
    Vi(crate::vi::ViSession),
    NvDriver(crate::nvdrv::NvClient),
    Time(crate::time_service::TimeObject),
    /// Client endpoint referring to a shared server object stored in `sessions`.
    Alias(u64),
    ParentalSession {
        initialized: bool,
    },
    Closed,
    Domain,
    Applet(crate::applet::AppletObject),
    ApmSession,
    Logger(crate::logging::Logger),
    NamedService(String),
    FileSystem(crate::fsp_object::FsSessionObject),
}

#[derive(Debug, Clone)]
pub struct HorizonSession {
    pub id: u64,
    pub target: SessionTarget,
}

#[derive(Debug)]
pub struct HorizonKernel {
    gpu_ioctl_backend: Option<Box<dyn crate::nvdrv::backend::NvGpuIoctlBackend>>,
    gpu_poll_cursor: Option<(u64, u32)>,
    /// Bounded protocol metadata only: never retain guest payloads, keys or buffer data.
    pub ipc_diagnostics: std::collections::VecDeque<(String, u32, u32)>,
    pub profile: HorizonProfileId,
    processes: BTreeMap<u64, HorizonProcess>,
    threads: BTreeMap<u64, HorizonThread>,
    next_process_id: u64,
    next_thread_id: u64,
    sessions: BTreeMap<u64, HorizonSession>,
    domains: BTreeMap<u64, crate::domain::Domain>,
    synchronization_waits: BTreeMap<u64, SynchronizationWait>,
    mutex_waits: Vec<mutex::MutexWait>,
    condition_waits: Vec<condition::ConditionWait>,
    next_session_id: u64,
    hid_shared_memory: BTreeMap<u64, crate::shared_memory::SharedMemory>,
    hid_service: crate::hid_service::SharedHidService,
    hid_primary_rings: BTreeMap<u64, crate::hid::ring::NpadRing>,
    hid_primary_last_sample_ns: BTreeMap<u64, u64>,
    pub services: ServiceRegistry,
    pub scheduler: Scheduler,
    pub fs: HorizonFs,
    pub hid: HidState,
    pub time: TimeManager,
    pub display: DisplayManager,
    pub logging: crate::logging::LogManager,
    pub performance: crate::apm::SharedPerformanceState,
    pub addons: crate::aoc::AddOnCatalog,
    pub bsd: crate::bsd::BsdManager,
}

impl HorizonKernel {
    pub fn new(profile: HorizonProfileId) -> Self {
        let mut services = ServiceRegistry::default();
        services.register(SmService);
        services.register(crate::bsd::BsdUserService);
        for privilege in [
            crate::vi::Privilege::Application,
            crate::vi::Privilege::System,
            crate::vi::Privilege::Manager,
        ] {
            services.register(crate::vi::ViService(privilege));
        }
        services.register(crate::nvdrv::NvService);
        services.register(crate::nifm::NifmUserService);
        let hid_service = crate::hid_service::SharedHidService::default();
        services.register(hid_service.clone());
        services.register(crate::time_service::TimeService);
        services.register(crate::logging::LogService);
        services.register(crate::applet::AppletService);
        services.register(crate::aoc::AddOnService);
        services.register(crate::parental::ParentalService);
        let performance = crate::apm::SharedPerformanceState::default();
        services.register(crate::apm::ApmService(performance.clone()));
        services.register(FspSrvService::default());
        Self {
            gpu_ioctl_backend: None,
            gpu_poll_cursor: None,
            ipc_diagnostics: Default::default(),
            bsd: Default::default(),
            profile,
            processes: BTreeMap::new(),
            threads: BTreeMap::new(),
            next_process_id: 1,
            next_thread_id: 1,
            sessions: BTreeMap::new(),
            domains: BTreeMap::new(),
            synchronization_waits: BTreeMap::new(),
            mutex_waits: Vec::new(),
            condition_waits: Vec::new(),
            next_session_id: 1,
            hid_shared_memory: BTreeMap::new(),
            hid_service,
            hid_primary_rings: BTreeMap::new(),
            hid_primary_last_sample_ns: BTreeMap::new(),
            services,
            scheduler: Scheduler::default(),
            fs: HorizonFs::default(),
            hid: HidState::default(),
            time: TimeManager::default(),
            display: DisplayManager::default(),
            logging: crate::logging::LogManager::default(),
            performance,
            addons: crate::aoc::AddOnCatalog::default(),
        }
    }

    /// Install the runtime queue executor. An existing executor cannot be
    /// replaced: doing so could abandon pending work and synchronization state.
    pub fn install_gpu_ioctl_backend(
        &mut self,
        backend: Box<dyn crate::nvdrv::backend::NvGpuIoctlBackend>,
    ) -> HorizonResult<()> {
        if self.gpu_ioctl_backend.is_some() {
            return Err(HorizonError::InvalidState(
                "GPU backend is already installed".into(),
            ));
        }
        self.gpu_ioctl_backend = Some(backend);
        Ok(())
    }

    /// Round-robin bounded channel polling, including while CPU threads wait.
    /// Borrow current owner memory/mappings; never execute a detached snapshot.
    pub fn poll_gpu_process(
        &mut self,
        process_id: u64,
        maximum_channels: usize,
    ) -> HorizonResult<usize> {
        if maximum_channels == 0 || maximum_channels > 4096 {
            return Err(HorizonError::InvalidState(
                "invalid GPU polling budget".into(),
            ));
        }
        let process = self
            .processes
            .get_mut(&process_id)
            .ok_or_else(|| HorizonError::InvalidState("GPU process does not exist".into()))?;
        if process.state == ProcessState::Exited {
            return Ok(0);
        }
        let Some(backend) = self.gpu_ioctl_backend.as_mut() else {
            return Ok(0);
        };
        let cursor = self.gpu_poll_cursor;
        let mut visited = 0;
        let mut retired = 0;
        // Two ordered passes wrap around the cursor without allocating a list
        // of every open channel. Each visit grants only one descriptor's work.
        for wrapped in [false, true] {
            for (&session_id, session) in &self.sessions {
                let SessionTarget::NvDriver(client) = &session.target else {
                    continue;
                };
                if !client.is_authenticated_owner(process.application.resource_user_id) {
                    continue;
                }
                for identity in client.channel_identities() {
                    let key = (session_id, identity.descriptor());
                    let after_cursor = cursor.is_none_or(|cursor| key > cursor);
                    if after_cursor == wrapped {
                        continue;
                    }
                    if visited == maximum_channels {
                        return Ok(retired);
                    }
                    visited += 1;
                    self.gpu_poll_cursor = Some(key);
                    if client
                        .channel_map_descriptor(identity.descriptor())
                        .is_none()
                    {
                        continue;
                    }
                    if let Some(gpu) = client.channel_address_space(identity.descriptor()) {
                        let count = backend.poll_channel(&identity, &mut process.memory, gpu, 1)?;
                        if count > 1 {
                            return Err(HorizonError::InvalidState(
                                "GPU backend exceeded polling budget".into(),
                            ));
                        }
                        retired += count;
                    }
                }
            }
        }
        Ok(retired)
    }

    /// Whether this application explicitly accepts an active FullKey controller
    /// in player slot zero. This is policy, not a connected-device assertion.
    pub fn primary_npad_enabled(&self, process_id: u64) -> HorizonResult<bool> {
        let resource_id = self.process(process_id)?.application.resource_user_id;
        self.hid_service.primary_full_key_allowed(resource_id)
    }

    /// Publish a caller-scheduled input tick to the retained application HID
    /// memory. No buffer is allocated before the application requests one.
    /// Currently scoped to the supplied 22.1.0 profile and player-zero FullKey.
    pub fn publish_primary_npad(&mut self, process_id: u64) -> HorizonResult<bool> {
        if self.profile.0 != "22.1.0" {
            return Err(HorizonError::InvalidState(
                "Npad publication requires the 22.1.0 layout profile".into(),
            ));
        }
        let identity = self.process(process_id)?.application.resource_user_id;
        let Some(memory) = self.hid_shared_memory.get(&identity) else {
            return Ok(false);
        };
        let allowed = self.hid_service.primary_full_key_allowed(identity)?;
        let state = if allowed {
            self.hid.primary_npad
        } else {
            crate::hid::NpadState::default()
        };
        // SharedMemoryFormat.Npad[0], then NpadInternalState.FullKeyLifo.
        // These are guest wire offsets, never guest virtual addresses.
        const NPAD_BASE: usize = 0x9a00;
        self.hid_primary_rings
            .entry(identity)
            .or_default()
            .publish_full_key(memory, NPAD_BASE, state)?;
        self.hid_service
            .primary_publication_completed(identity, state.connected)?;
        Ok(true)
    }

    /// Host sampling policy: at most one current-state sample per 8 ms poll.
    /// Missed intervals are coalesced, never filled with invented historical input.
    /// This interval is an emulator policy, not a measured firmware property.
    pub fn poll_primary_npad(&mut self, process_id: u64) -> HorizonResult<bool> {
        if self.profile.0 != "22.1.0" {
            return Ok(false);
        }
        let identity = self.process(process_id)?.application.resource_user_id;
        if !self.hid_shared_memory.contains_key(&identity) {
            return Ok(false);
        }
        let now = self.time.steady_clock().time_point_ns;
        if self
            .hid_primary_last_sample_ns
            .get(&identity)
            .is_some_and(|last| now.saturating_sub(*last) < 8_000_000)
        {
            return Ok(false);
        }
        if self.publish_primary_npad(process_id)? {
            self.hid_primary_last_sample_ns.insert(identity, now);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn create_process(&mut self, program_id: u64, name: impl Into<String>) -> u64 {
        let id = self.next_process_id;
        self.next_process_id += 1;
        self.processes
            .insert(id, HorizonProcess::new(id, program_id, name));
        id
    }

    pub fn process(&self, id: u64) -> HorizonResult<&HorizonProcess> {
        self.processes
            .get(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("process {id} does not exist")))
    }

    pub fn process_mut(&mut self, id: u64) -> HorizonResult<&mut HorizonProcess> {
        self.processes
            .get_mut(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("process {id} does not exist")))
    }

    /// Borrow the authenticated channel's canonical mappings and owning process
    /// memory together. Session aliases must resolve before borrowing; copying
    /// the mapping table would disconnect execution from later map/unmap calls.
    pub fn with_gpu_channel<T>(
        &mut self,
        process_id: u64,
        session_handle: crate::handle::Handle,
        descriptor: u32,
        action: impl FnOnce(
            crate::nvdrv::NvGpuChannelIdentity,
            &mut crate::memory::AddressSpace,
            &crate::nvdrv::address_space::NvGpuAddressSpace,
        ) -> T,
    ) -> HorizonResult<T> {
        let process = self.process(process_id)?;
        if process.state == ProcessState::Exited {
            return Err(HorizonError::InvalidState(
                "GPU owner process has exited".into(),
            ));
        }
        let owner = process.application.resource_user_id;
        let KernelObject::Session(client_id) = process.handles.get(session_handle)? else {
            return Err(HorizonError::InvalidHandle(session_handle.0));
        };
        let server = self.resolve_session(*client_id)?;
        let Some(HorizonSession {
            target: SessionTarget::NvDriver(client),
            ..
        }) = self.sessions.get(&server)
        else {
            return Err(HorizonError::InvalidState(
                "GPU execution requires an nvdrv session".into(),
            ));
        };
        if !client.is_authenticated_owner(owner) {
            return Err(HorizonError::InvalidState(
                "GPU execution owner mismatch".into(),
            ));
        }
        let identity = client.channel_identity(descriptor).ok_or_else(|| {
            HorizonError::InvalidState("GPU channel descriptor is not live".into())
        })?;
        if client.channel_map_descriptor(descriptor).is_none() {
            return Err(HorizonError::InvalidState(
                "GPU channel has no nvmap owner".into(),
            ));
        }
        let gpu = client.channel_address_space(descriptor).ok_or_else(|| {
            HorizonError::InvalidState("GPU channel has no bound address space".into())
        })?;
        let process = self.processes.get_mut(&process_id).unwrap();
        Ok(action(identity, &mut process.memory, gpu))
    }

    pub fn start_process(&mut self, id: u64) -> HorizonResult<()> {
        self.process_mut(id)?.state = ProcessState::Running;
        Ok(())
    }

    pub fn exit_process(&mut self, id: u64) -> HorizonResult<()> {
        if self.process(id)?.state == ProcessState::Exited {
            return Ok(());
        }
        let resource_id = self.process(id)?.application.resource_user_id;
        if let Some(backend) = self.gpu_ioctl_backend.as_mut() {
            for session in self.sessions.values() {
                if let SessionTarget::NvDriver(client) = &session.target {
                    if client.is_authenticated_owner(resource_id) {
                        for identity in client.channel_identities() {
                            backend.close_channel(identity)?;
                        }
                    }
                }
            }
        }
        self.display.release_managed_layers(resource_id);
        let thread_ids = self.process(id)?.thread_ids.clone();
        self.mutex_waits.retain(|wait| wait.process_id != id);
        self.condition_waits.retain(|wait| wait.process_id != id);
        for thread_id in thread_ids {
            self.scheduler.remove(thread_id);
            if let Some(thread) = self.threads.get_mut(&thread_id) {
                thread.state = ThreadState::Exited;
            }
        }
        self.process_mut(id)?.state = ProcessState::Exited;
        Ok(())
    }

    pub fn create_thread(
        &mut self,
        process_id: u64,
        entry: u64,
        stack_top: u64,
        priority: i32,
    ) -> HorizonResult<(u64, crate::handle::Handle)> {
        let core = self.process(process_id)?.ideal_core;
        self.create_thread_on_core(process_id, entry, stack_top, priority, core)
    }

    /// Admit guest affinity before allocating a handle or TLS slot.
    pub fn create_thread_on_core(
        &mut self,
        process_id: u64,
        entry: u64,
        stack_top: u64,
        priority: i32,
        core: Option<u8>,
    ) -> HorizonResult<(u64, crate::handle::Handle)> {
        if !self.processes.contains_key(&process_id) {
            return Err(HorizonError::InvalidState(format!(
                "process {process_id} does not exist"
            )));
        }
        let id = self.next_thread_id;
        let process = self.process(process_id)?;
        let initial_affinity = match (process.scheduling, core) {
            (Some(grants), Some(core)) => {
                if core >= 4
                    || grants.core_mask & (1u64 << core) == 0
                    || !(4..=63).contains(&priority)
                    || grants.priority_mask & (1u64 << priority) == 0
                {
                    return Err(HorizonError::InvalidState(
                        "thread exceeds process scheduling grants".into(),
                    ));
                }
                Some(crate::thread::ThreadAffinity {
                    ideal_core: i32::from(core),
                    mask: 1u64 << core,
                })
            }
            _ => None,
        };
        let handle = self
            .process_mut(process_id)?
            .handles
            .insert(KernelObject::Thread(id))?;
        let tls = match self.process_mut(process_id)?.allocate_tls() {
            Ok(address) => address,
            Err(error) => {
                self.process_mut(process_id)?.handles.close(handle)?;
                return Err(error);
            }
        };
        let mut thread = HorizonThread::new(id, process_id, entry, stack_top, priority);
        thread.affinity = initial_affinity;
        thread.context.tpidrro_el0 = tls;
        self.next_thread_id += 1;
        self.threads.insert(id, thread);
        self.process_mut(process_id)?.thread_ids.push(id);
        Ok((id, handle))
    }

    pub fn thread(&self, id: u64) -> HorizonResult<&HorizonThread> {
        self.threads
            .get(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("thread {id} does not exist")))
    }

    pub fn start_thread(&mut self, id: u64) -> HorizonResult<()> {
        let thread = self
            .threads
            .get_mut(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("thread {id} does not exist")))?;
        if thread.state != ThreadState::Created && thread.state != ThreadState::Waiting {
            return Err(HorizonError::InvalidState(format!(
                "thread {id} cannot be started from {:?}",
                thread.state
            )));
        }
        thread.state = ThreadState::Ready;
        let priority = thread.priority;
        self.scheduler.enqueue_with_priority(id, priority);
        Ok(())
    }

    pub fn exit_thread(&mut self, id: u64) -> HorizonResult<()> {
        if self.mutex_waits.iter().any(|wait| wait.owner == id) {
            return Err(HorizonError::InvalidState(
                "exiting a mutex owner with waiters is not implemented".into(),
            ));
        }
        self.mutex_waits.retain(|wait| wait.thread_id != id);
        self.condition_waits.retain(|wait| wait.thread_id != id);
        self.scheduler.remove(id);
        self.threads
            .get_mut(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("thread {id} does not exist")))?
            .state = ThreadState::Exited;
        self.recompute_mutex_priorities();
        Ok(())
    }

    /// Signal only the condition queue associated with this process and key.
    pub fn signal_process_wide_key(
        &mut self,
        process_id: u64,
        address: u64,
        count: i32,
    ) -> HorizonResult<()> {
        if address % 4 != 0 {
            return Err(HorizonError::InvalidAddress(address));
        }
        let process = self.process(process_id)?;
        if process.thread_ids.iter().any(|id| {
            self.threads.get(id).is_some_and(|thread| {
                thread.state == ThreadState::Waiting
                    && !self.synchronization_waits.contains_key(id)
                    && !self.mutex_waits.iter().any(|wait| wait.thread_id == *id)
                    && !self
                        .condition_waits
                        .iter()
                        .any(|wait| wait.thread_id == *id)
            })
        }) {
            return Err(HorizonError::InvalidState(
                "condition signaling with waiting threads is not implemented".into(),
            ));
        }
        self.signal_condition(process_id, address, count)
    }

    /// Creates a process-local session handle for a registered Horizon service.
    pub fn connect_service(
        &mut self,
        process_id: u64,
        service_name: &str,
    ) -> HorizonResult<crate::handle::Handle> {
        if !self.services.names().any(|name| name == service_name) {
            return Err(HorizonError::UnknownService(service_name.to_owned()));
        }
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
        self.sessions.insert(
            id,
            HorizonSession {
                id,
                target: if service_name == "nvdrv" {
                    SessionTarget::NvDriver(Default::default())
                } else if service_name == "bsd:u" {
                    SessionTarget::Bsd(Default::default())
                } else {
                    SessionTarget::NamedService(service_name.to_owned())
                },
            },
        );
        self.process_mut(process_id)?
            .handles
            .insert(KernelObject::Session(id))
    }

    fn connect_fs_object(
        &mut self,
        process_id: u64,
        object: crate::fsp_object::FsSessionObject,
    ) -> HorizonResult<crate::handle::Handle> {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
        self.sessions.insert(
            id,
            HorizonSession {
                id,
                target: SessionTarget::FileSystem(object),
            },
        );
        self.process_mut(process_id)?
            .handles
            .insert(KernelObject::Session(id))
    }

    fn materialize_returned_object(
        &mut self,
        process_id: u64,
        object: crate::ipc::ReturnedObject,
    ) -> HorizonResult<crate::handle::Handle> {
        match object {
            crate::ipc::ReturnedObject::ParentalSession { initialized } => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::ParentalSession { initialized },
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::AppletMessageEvent
            | crate::ipc::ReturnedObject::ReadableEvent(_)
            | crate::ipc::ReturnedObject::HidSharedMemory { .. }
            | crate::ipc::ReturnedObject::AddOnListEvent => Err(HorizonError::InvalidState(
                "event requires copy-handle transport".into(),
            )),
            crate::ipc::ReturnedObject::Applet(object) => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Applet(object),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::ApmSession => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::ApmSession,
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::Logger {
                process_id: logger_process_id,
            } => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Logger(crate::logging::Logger {
                            process_id: logger_process_id,
                            packet_count: 0,
                            byte_count: 0,
                        }),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::Nifm(object) => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Nifm(object),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::Hid(object) => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Hid(object),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::Vi(object) => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Vi(object),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::Time(object) => {
                let id = self.next_session_id;
                let handle = self
                    .process_mut(process_id)?
                    .handles
                    .insert(KernelObject::Session(id))?;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                self.sessions.insert(
                    id,
                    HorizonSession {
                        id,
                        target: SessionTarget::Time(object),
                    },
                );
                Ok(handle)
            }
            crate::ipc::ReturnedObject::NamedService(name) => {
                self.connect_service(process_id, &name)
            }
            object @ (crate::ipc::ReturnedObject::FileSystem { .. }
            | crate::ipc::ReturnedObject::File { .. }
            | crate::ipc::ReturnedObject::Directory { .. }
            | crate::ipc::ReturnedObject::Storage { .. }) => {
                let target = self.returned_fs_object(object)?;
                self.connect_fs_object(process_id, target)
            }
        }
    }

    /// Native sessions and domain children use the same filesystem validation.
    fn returned_fs_object(
        &self,
        object: crate::ipc::ReturnedObject,
    ) -> HorizonResult<crate::fsp_object::FsSessionObject> {
        use crate::fsp_object::FsSessionObject;
        match object {
            crate::ipc::ReturnedObject::FileSystem { mount } => {
                self.fs
                    .mounts
                    .get(&exosphere_vfs::MountId(mount.clone()))
                    .map_err(|error| HorizonError::FileSystem(error.to_string()))?;
                Ok(FsSessionObject::FileSystem { mount })
            }
            crate::ipc::ReturnedObject::File { mount, path, mode } => {
                let path = exosphere_vfs::GuestPath::parse(&path)
                    .map_err(|error| HorizonError::FileSystem(error.to_string()))?;
                Ok(FsSessionObject::File {
                    mount,
                    path,
                    mode: exosphere_vfs::OpenMode(mode),
                })
            }
            crate::ipc::ReturnedObject::Directory {
                mount,
                path,
                filter,
            } => {
                let path = exosphere_vfs::GuestPath::parse(&path)
                    .map_err(|error| HorizonError::FileSystem(error.to_string()))?;
                Ok(FsSessionObject::Directory {
                    mount,
                    path,
                    filter,
                    cursor: 0,
                })
            }
            crate::ipc::ReturnedObject::Storage { name, writable } => {
                self.fs.storage(&name)?;
                Ok(FsSessionObject::Storage { name, writable })
            }
            _ => Err(HorizonError::InvalidState(
                "expected a filesystem object".into(),
            )),
        }
    }

    /// Exchange the actual guest TLS command buffer with the existing IPC dispatcher.
    pub fn send_guest_sync_request(
        &mut self,
        process_id: u64,
        thread_id: u64,
        handle: crate::handle::Handle,
    ) -> HorizonResult<()> {
        use crate::memory::MemoryPermission;
        const COMMAND_BUFFER_SIZE: usize = 0x100;
        let thread = self.thread(thread_id)?;
        if thread.process_id != process_id {
            return Err(HorizonError::InvalidState(
                "IPC thread belongs to another process".into(),
            ));
        }
        let address = thread.context.tpidrro_el0;
        let memory = &self.process(process_id)?.memory;
        memory.validate_access(address, COMMAND_BUFFER_SIZE, MemoryPermission::READ_WRITE)?;
        let mut request = vec![0; COMMAND_BUFFER_SIZE];
        memory.read(address, &mut request)?;
        self.thread_mut(thread_id)?.ipc_buffer = request;
        self.send_sync_request(process_id, thread_id, handle)?;
        let response = &self.thread(thread_id)?.ipc_buffer;
        if response.len() > COMMAND_BUFFER_SIZE {
            return Err(HorizonError::IpcProtocol(
                "response exceeds TLS command buffer".into(),
            ));
        }
        let mut output = [0u8; COMMAND_BUFFER_SIZE];
        output[..response.len()].copy_from_slice(response);
        self.process_mut(process_id)?.memory.write(address, &output)
    }

    /// Dispatches the synthetic thread IPC buffer through a session and replaces it with the response.
    ///
    /// HIPC descriptors are validated against the calling process address space before the service
    /// sees them. Send/pointer buffers are copied in, receive buffers are copied out only after a
    /// successful service dispatch, and exchange buffers support both directions.
    pub fn send_sync_request(
        &mut self,
        process_id: u64,
        thread_id: u64,
        handle: crate::handle::Handle,
    ) -> HorizonResult<()> {
        let client_session_id = match self.process(process_id)?.handles.get(handle)? {
            KernelObject::Session(id) => *id,
            _ => return Err(HorizonError::InvalidHandle(handle.0)),
        };
        let session_id = self.resolve_session(client_session_id)?;
        let request_bytes = self
            .threads
            .get(&thread_id)
            .ok_or_else(|| {
                HorizonError::InvalidState(format!("thread {thread_id} does not exist"))
            })?
            .ipc_buffer
            .clone();

        let hipc = crate::hipc::parse(&request_bytes)?;
        if hipc.header.message_type == crate::hipc::HipcMessageType::Release {
            if !hipc.raw_data.is_empty()
                || hipc.header.pointer_count != 0
                || hipc.header.send_count != 0
                || hipc.header.receive_count != 0
                || hipc.header.exchange_count != 0
                || hipc.header.has_special_header
            {
                return Err(HorizonError::IpcProtocol(
                    "Release must not carry command data".into(),
                ));
            }
            // Close the server object, not the client's handle: the guest can still
            // perform CloseHandle afterward. Release carries no CMIF response body.
            self.release_session(client_session_id)?;
            self.thread_mut(thread_id)?.ipc_buffer = vec![0; 8];
            return Ok(());
        }

        let is_manager = matches!(
            hipc.header.message_type,
            crate::hipc::HipcMessageType::InvokeManagerMethod
                | crate::hipc::HipcMessageType::Invoke2ManagerMethodOld
                | crate::hipc::HipcMessageType::Invoke2ManagerMethod
        );
        if !is_manager
            && !matches!(
                hipc.header.message_type,
                crate::hipc::HipcMessageType::InvokeMethod
                    | crate::hipc::HipcMessageType::Invoke2MethodOld
                    | crate::hipc::HipcMessageType::Invoke2Method
            )
        {
            return Err(HorizonError::IpcProtocol(
                "unsupported HIPC message type".into(),
            ));
        }
        let domain_request = if self.domains.contains_key(&session_id) && !is_manager {
            let raw_offset = hipc.raw_data.as_ptr() as usize - request_bytes.as_ptr() as usize;
            Some(crate::domain::parse(
                hipc.raw_data,
                (16 - raw_offset % 16) % 16,
            )?)
        } else {
            None
        };
        if let Some(request) = &domain_request {
            if request.release {
                if hipc.header.has_special_header
                    || !hipc.pointers.is_empty()
                    || !hipc.send_buffers.is_empty()
                    || !hipc.receive_buffers.is_empty()
                    || !hipc.exchange_buffers.is_empty()
                    || !hipc.receive_list.is_empty()
                {
                    return Err(HorizonError::IpcProtocol(
                        "domain Release must not carry handles or buffers".into(),
                    ));
                }
                let domain = self.domains.get_mut(&session_id).unwrap();
                domain
                    .objects
                    .remove(&request.target)
                    .ok_or_else(|| HorizonError::InvalidState("unknown domain object".into()))?;
                self.collect_transfer_reservations();
                self.thread_mut(thread_id)?.ipc_buffer =
                    crate::hipc::encode_simple_cmif_response(&crate::domain::encode(&[], &[]));
                return Ok(());
            }
        }
        let target = if let Some(request) = &domain_request {
            self.domains[&session_id]
                .objects
                .get(&request.target)
                .ok_or_else(|| HorizonError::InvalidState("unknown domain object".into()))?
        } else {
            &self
                .sessions
                .get(&session_id)
                .ok_or(HorizonError::InvalidHandle(handle.0))?
                .target
        };
        let target_name = match target {
            SessionTarget::Bsd(_) => "bsd:u".into(),
            SessionTarget::Alias(_) => {
                return Err(HorizonError::InvalidState("nested session alias".into()));
            }
            SessionTarget::Closed => return Err(HorizonError::InvalidHandle(handle.0)),
            SessionTarget::Domain => "HIPC manager".into(),
            SessionTarget::Logger(_) => "ILogger".into(),
            SessionTarget::Nifm(object) => object.name().into(),
            SessionTarget::Hid(object) => object.name().into(),
            SessionTarget::Vi(object) => object.name().into(),
            SessionTarget::NvDriver(_) => "nvdrv".into(),
            SessionTarget::Time(object) => object.name().into(),
            SessionTarget::ApmSession => "IApmSession".into(),
            SessionTarget::ParentalSession { .. } => "IParentalControlService".into(),
            SessionTarget::Applet(object) => object.name().into(),
            SessionTarget::NamedService(name) => name.clone(),
            SessionTarget::FileSystem(object) => match object {
                crate::fsp_object::FsSessionObject::FileSystem { .. } => "IFileSystem".to_owned(),
                crate::fsp_object::FsSessionObject::File { .. } => "IFile".to_owned(),
                crate::fsp_object::FsSessionObject::Directory { .. } => "IDirectory".to_owned(),
                crate::fsp_object::FsSessionObject::Storage { .. } => "IStorage".to_owned(),
            },
        };

        let cmif = crate::cmif::parse_request(
            domain_request
                .as_ref()
                .map_or(hipc.raw_data, |request| request.cmif),
        )
        .map_err(|error| {
            let magic_offset = hipc.raw_data.windows(4).position(|bytes| bytes == b"SFCI");
            HorizonError::IpcProtocol(format!(
                "{error}; message={:?}, raw_bytes={}, SFCI_offset={magic_offset:?}",
                hipc.header.message_type,
                hipc.raw_data.len()
            ))
        })?;
        if matches!(
            hipc.header.message_type,
            crate::hipc::HipcMessageType::InvokeManagerMethod
                | crate::hipc::HipcMessageType::Invoke2ManagerMethodOld
                | crate::hipc::HipcMessageType::Invoke2ManagerMethod
        ) {
            if !matches!(cmif.header.method_id, 0 | 2 | 3 | 4) {
                return Err(HorizonError::UnknownCommand {
                    service: "HIPC manager".into(),
                    command_id: cmif.header.method_id,
                });
            }
            if hipc.header.has_special_header
                || !hipc.pointers.is_empty()
                || !hipc.send_buffers.is_empty()
                || !hipc.receive_buffers.is_empty()
                || !hipc.exchange_buffers.is_empty()
                || !hipc.receive_list.is_empty()
            {
                return Err(HorizonError::IpcProtocol(
                    "HIPC manager request must not carry descriptors or handles".into(),
                ));
            }
            if matches!(cmif.header.method_id, 2 | 4) {
                if cmif.header.method_id == 4 {
                    // CloneEx adds a u32 session-manager tag. The serial host
                    // dispatcher has one manager; tags do not select another
                    // server object or justify duplicating mutable client state.
                    // Count the 16 total padding bytes too, so trailing padding
                    // cannot masquerade as a missing argument.
                    if hipc.raw_data.len() < 16 + 16 + 4 {
                        return Err(HorizonError::IpcProtocol(
                            "CloneCurrentObjectEx tag is truncated".into(),
                        ));
                    }
                    let tag = cmif.arguments.get(..4).ok_or_else(|| {
                        HorizonError::IpcProtocol("CloneCurrentObjectEx requires a u32 tag".into())
                    })?;
                    let _session_manager_tag = u32::from_le_bytes(tag.try_into().unwrap());
                }
                let clone = self.clone_session(process_id, client_session_id)?;
                let response = crate::cmif::encode_response(
                    crate::cmif::CmifOutHeader {
                        version: cmif.header.version,
                        result: 0,
                        interface_id: 0,
                    },
                    &[],
                );
                self.thread_mut(thread_id)?.ipc_buffer =
                    crate::hipc::encode_cmif_response_with_handles(&response, &[], &[clone.0])?;
                return Ok(());
            }
            // HID input pointers are copied into bounded host-dispatch staging.
            // Other endpoints retain zero capacity until their pointer contract is audited.
            let payload = if cmif.header.method_id == 0 {
                if self.domains.contains_key(&session_id) {
                    return Err(HorizonError::InvalidState(
                        "session is already a domain".into(),
                    ));
                }
                let root = std::mem::replace(
                    &mut self.sessions.get_mut(&session_id).unwrap().target,
                    SessionTarget::Domain,
                );
                self.domains
                    .insert(session_id, crate::domain::Domain::new(root));
                1u32.to_le_bytes().to_vec()
            } else {
                (if target_name == "hid" {
                    crate::hid_service::POINTER_BUFFER_SIZE
                } else {
                    0u16
                })
                .to_le_bytes()
                .to_vec()
            };
            let response = crate::cmif::encode_response(
                crate::cmif::CmifOutHeader {
                    version: cmif.header.version,
                    result: 0,
                    interface_id: 0,
                },
                &payload,
            );
            self.thread_mut(thread_id)?.ipc_buffer =
                crate::hipc::encode_simple_cmif_response(&response);
            return Ok(());
        }
        if target_name == "hid" {
            let pointer_bytes: u64 = hipc
                .pointers
                .iter()
                .map(|p| u64::from(p.size))
                .chain(hipc.receive_list.iter().map(|p| u64::from(p.size)))
                .sum();
            if pointer_bytes > u64::from(crate::hid_service::POINTER_BUFFER_SIZE) {
                return Err(HorizonError::IpcProtocol(
                    "HID pointer staging capacity exceeded".into(),
                ));
            }
        }
        let buffers = self.materialize_ipc_buffers(process_id, &hipc)?;
        if target_name == "IApplicationDisplayService" && cmif.header.method_id == 2101 {
            if !hipc.handles.copy_handles.is_empty() || !hipc.handles.move_handles.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "SetLayerScalingMode does not accept handles".into(),
                ));
            }
            if domain_request.is_none() && hipc.raw_data.len() < 16 + 16 + 16 {
                return Err(HorizonError::IpcProtocol(
                    "SetLayerScalingMode arguments are truncated".into(),
                ));
            }
        }
        if target_name == "IHOSBinderDriverRelay" && matches!(cmif.header.method_id, 0..=3) {
            if !hipc.handles.copy_handles.is_empty() || !hipc.handles.move_handles.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "Binder relay commands do not accept input handles".into(),
                ));
            }
            let argument_size = if cmif.header.method_id == 2 { 8 } else { 12 };
            if domain_request.is_none() && hipc.raw_data.len() < 16 + 16 + argument_size {
                return Err(HorizonError::IpcProtocol(
                    "Binder relay arguments are truncated".into(),
                ));
            }
            if matches!(cmif.header.method_id, 0 | 3) {
                // Validate the declared reply mapping before a Binder operation
                // can commit queue state, not only while copying its response.
                for buffer in &buffers {
                    if buffer.direction == crate::ipc::IpcBufferDirection::Out {
                        let size = usize::try_from(buffer.capacity)
                            .map_err(|_| HorizonError::InvalidAddress(buffer.address))?;
                        self.process(process_id)?.memory.validate_access(
                            buffer.address,
                            size,
                            crate::memory::MemoryPermission::WRITE,
                        )?;
                    }
                }
            }
        }
        if target_name == "IApplicationDisplayService" && cmif.header.method_id == 2020 {
            if hipc.handles.process_id.is_none()
                || !hipc.handles.copy_handles.is_empty()
                || !hipc.handles.move_handles.is_empty()
            {
                return Err(HorizonError::IpcProtocol(
                    "OpenLayer requires PID framing and no handles".into(),
                ));
            }
            if domain_request.is_none() && hipc.raw_data.len() < 16 + 16 + 80 {
                return Err(HorizonError::IpcProtocol(
                    "OpenLayer arguments are truncated".into(),
                ));
            }
            for buffer in &buffers {
                if buffer.direction == crate::ipc::IpcBufferDirection::Out
                    && buffer.capacity >= crate::graphic_producer::NATIVE_WINDOW_SIZE as u64
                {
                    self.process(process_id)?.memory.validate_access(
                        buffer.address,
                        crate::graphic_producer::NATIVE_WINDOW_SIZE,
                        crate::memory::MemoryPermission::WRITE,
                    )?;
                }
            }
        }
        if target_name == "IApplicationFunctions"
            && cmif.header.method_id == 22
            && domain_request.is_none()
            && hipc.raw_data.len() < 16 + 16 + 4
        {
            // Native CMIF arguments include trailing HIPC padding. Do not read
            // that padding as a missing application termination-result argument.
            return Err(HorizonError::IpcProtocol(
                "SetTerminateResult argument is truncated".into(),
            ));
        }
        let bsd_transfer = if target_name == "bsd:u" && matches!(cmif.header.method_id, 0 | 1) {
            if hipc.handles.process_id.is_none()
                || !hipc.handles.move_handles.is_empty()
                || hipc.handles.copy_handles.len() != usize::from(cmif.header.method_id == 0)
            {
                return Err(HorizonError::IpcProtocol(
                    "BSD bootstrap requires PID and the expected copy handles".into(),
                ));
            }
            if cmif.header.method_id == 0 {
                let raw = cmif.arguments.get(..48).ok_or_else(|| {
                    HorizonError::IpcProtocol("BSD registration arguments are truncated".into())
                })?;
                let requested = u64::from_le_bytes(raw[40..48].try_into().unwrap());
                let process = self.process(process_id)?;
                let object = process
                    .handles
                    .get(crate::handle::Handle(hipc.handles.copy_handles[0]))?
                    .clone();
                match &object {
                    KernelObject::TransferMemory { owner, base, size }
                        if *owner == process_id
                            && *size == requested
                            && requested != 0
                            && process.memory.transfer_metadata(*base)
                                == Some((crate::memory::MemoryPermission::NONE, 1)) =>
                    {
                        Some(object)
                    }
                    _ => {
                        return Err(HorizonError::IpcProtocol(
                            "BSD transfer owner/size/permission mismatch".into(),
                        ));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        if target_name == "nvdrv" && matches!(cmif.header.method_id, 0 | 1 | 2 | 4 | 11 | 12) {
            if !hipc.handles.copy_handles.is_empty() || !hipc.handles.move_handles.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "nvdrv device commands do not accept handles".into(),
                ));
            }
            let argument_size = match cmif.header.method_id {
                0 => 0,
                2 => 4,
                _ => 8,
            };
            if matches!(cmif.header.method_id, 1 | 11 | 12) {
                // Device operations may mutate allocation state. Validate output
                // permissions before dispatch so failed writeback cannot leak it.
                for buffer in &buffers {
                    if matches!(
                        buffer.direction,
                        crate::ipc::IpcBufferDirection::Out | crate::ipc::IpcBufferDirection::InOut
                    ) && buffer.capacity != 0
                    {
                        let size = usize::try_from(buffer.capacity).map_err(|_| {
                            HorizonError::IpcProtocol("nvdrv output size exceeds host range".into())
                        })?;
                        self.process(process_id)?.memory.validate_access(
                            buffer.address,
                            size,
                            crate::memory::MemoryPermission::WRITE,
                        )?;
                    }
                }
            }
            // Native HIPC padding is not part of the CMIF argument structure.
            if domain_request.is_none() && hipc.raw_data.len() < 32 + argument_size {
                return Err(HorizonError::IpcProtocol(
                    "nvdrv device arguments are truncated".into(),
                ));
            }
        }
        let nv_transfer = if target_name == "nvdrv" && cmif.header.method_id == 3 {
            if hipc.handles.copy_handles.len() != 2 || !hipc.handles.move_handles.is_empty() {
                return Err(HorizonError::IpcProtocol(
                    "nvdrv Initialize requires two copy handles".into(),
                ));
            }
            let process = self.process(process_id)?;
            let process_handle = crate::handle::Handle(hipc.handles.copy_handles[0]);
            if process_handle.0 != 0xffff8001
                && process.handles.get(process_handle)? != &KernelObject::Process(process_id)
            {
                return Err(HorizonError::InvalidHandle(process_handle.0));
            }
            let object = process
                .handles
                .get(crate::handle::Handle(hipc.handles.copy_handles[1]))?
                .clone();
            let size = cmif.arguments.get(..4).ok_or_else(|| {
                HorizonError::IpcProtocol("nvdrv Initialize requires transfer size".into())
            })?;
            let requested = u64::from(u32::from_le_bytes(size.try_into().unwrap()));
            match &object {
                KernelObject::TransferMemory { owner, base, size }
                    if *owner == process_id
                        && *size == requested
                        && requested != 0
                        && process.memory.transfer_metadata(*base)
                            == Some((crate::memory::MemoryPermission::NONE, 1)) =>
                {
                    Some(object)
                }
                _ => {
                    return Err(HorizonError::IpcProtocol(
                        "nvdrv transfer owner/size/permission mismatch".into(),
                    ));
                }
            }
        } else {
            None
        };
        if target_name == "nvdrv" && cmif.header.method_id == 8 && hipc.handles.process_id.is_none()
        {
            return Err(HorizonError::IpcProtocol(
                "nvdrv SetAruid requires a PID descriptor".into(),
            ));
        }
        if target_name == "pctl:a"
            && matches!(cmif.header.method_id, 0 | 1)
            && hipc.handles.process_id.is_none()
        {
            return Err(HorizonError::IpcProtocol(
                "parental factory requires a PID descriptor".into(),
            ));
        }
        if target_name == "appletOE" && cmif.header.method_id == 0 {
            self.validate_applet_process_handle(process_id, &hipc)?;
        }
        if target_name == "hid"
            && matches!(cmif.header.method_id, 0 | 1 | 100 | 101 | 102 | 103 | 106)
        {
            let identity_offset = if matches!(cmif.header.method_id, 100 | 106) {
                8
            } else {
                0
            };
            if hipc.handles.process_id.is_none()
                || !hipc.handles.copy_handles.is_empty()
                || !hipc.handles.move_handles.is_empty()
            {
                return Err(HorizonError::IpcProtocol(
                    "HID request requires a PID descriptor and no handles".into(),
                ));
            }
            if domain_request.is_none() && hipc.raw_data.len() < 16 + 16 + identity_offset + 8 {
                return Err(HorizonError::IpcProtocol(
                    "HID request argument is truncated".into(),
                ));
            }
            let identity = cmif
                .arguments
                .get(identity_offset..identity_offset + 8)
                .ok_or_else(|| {
                    HorizonError::IpcProtocol("HID resource identity is truncated".into())
                })?;
            if u64::from_le_bytes(identity.try_into().unwrap())
                != self.process(process_id)?.application.resource_user_id
            {
                return Err(HorizonError::InvalidState(
                    "HID resource identity does not belong to caller".into(),
                ));
            }
        }
        let request = crate::ipc::ServiceRequest {
            service: target_name,
            command_id: cmif.header.method_id,
            values: Vec::new(),
            raw_payload: cmif.arguments.to_vec(),
            buffers,
        };
        let version = cmif.header.version;
        let diagnostic = (request.service.clone(), request.command_id);

        let response = {
            let target = if let Some(request) = &domain_request {
                self.domains
                    .get_mut(&session_id)
                    .unwrap()
                    .objects
                    .get_mut(&request.target)
                    .unwrap()
            } else {
                &mut self.sessions.get_mut(&session_id).unwrap().target
            };
            match target {
                SessionTarget::Bsd(client) => {
                    self.bsd
                        .dispatch(client, &request, bsd_transfer, process_id)?
                }
                SessionTarget::Alias(_) => {
                    return Err(HorizonError::InvalidState("nested session alias".into()));
                }
                SessionTarget::Closed => return Err(HorizonError::InvalidHandle(handle.0)),
                SessionTarget::Domain => {
                    return Err(HorizonError::IpcProtocol(
                        "domain request header required".into(),
                    ));
                }
                SessionTarget::Logger(logger) => self.logging.dispatch(logger, &request)?,
                SessionTarget::Nifm(object) => object.dispatch(&request)?,
                SessionTarget::Hid(object) => object.dispatch(&request)?,
                SessionTarget::Vi(object) => object.dispatch_owned(
                    &mut self.display,
                    self.processes[&process_id].application.resource_user_id,
                    &request,
                )?,
                SessionTarget::NvDriver(client) => {
                    let process = self.processes.get_mut(&process_id).unwrap();
                    // Only queue/object allocation and submission cross into the runtime.
                    // Mapping, initialization and other device commands retain
                    // their existing canonical nvdrv dispatch path.
                    let queue_descriptor =
                        if matches!(request.command_id, 1 | 11) && request.raw_payload.len() >= 8 {
                            let ioctl =
                                u32::from_le_bytes(request.raw_payload[4..8].try_into().unwrap());
                            ((request.command_id == 1
                                && (matches!(ioctl, 0xc020481a | 0xc0104809)
                                    || ioctl & 0xc000ffff == 0xc0004808))
                                || (request.command_id == 11 && ioctl == 0xc018481b))
                                .then(|| {
                                    u32::from_le_bytes(request.raw_payload[..4].try_into().unwrap())
                                })
                        } else {
                            None
                        };
                    if let (Some(backend), Some(descriptor)) =
                        (self.gpu_ioctl_backend.as_mut(), queue_descriptor)
                    {
                        if process.state == ProcessState::Exited {
                            return Err(HorizonError::InvalidState(
                                "GPU owner process has exited".into(),
                            ));
                        }
                        if !client.is_authenticated_owner(process.application.resource_user_id) {
                            return Err(HorizonError::InvalidState(
                                "GPU execution owner mismatch".into(),
                            ));
                        }
                        let identity = client.channel_identity(descriptor).ok_or_else(|| {
                            HorizonError::InvalidState("GPU channel descriptor is not live".into())
                        })?;
                        if client.channel_map_descriptor(descriptor).is_none() {
                            return Err(HorizonError::InvalidState(
                                "GPU channel has no nvmap owner".into(),
                            ));
                        }
                        let gpu = client.channel_address_space(descriptor).ok_or_else(|| {
                            HorizonError::InvalidState(
                                "GPU channel has no bound address space".into(),
                            )
                        })?;
                        // Strip native HIPC alignment padding, not ABI arguments.
                        let mut queue_request = request.clone();
                        queue_request.raw_payload.truncate(8);
                        backend.dispatch(identity, &mut process.memory, gpu, &queue_request)?
                    } else {
                        let closing_identity = if request.command_id == 2 {
                            request.raw_payload.get(..4).and_then(|bytes| {
                                let descriptor = u32::from_le_bytes(bytes.try_into().unwrap());
                                client
                                    .channel_identity(descriptor)
                                    .map(|identity| (descriptor, identity))
                            })
                        } else {
                            None
                        };
                        let response = client.dispatch_with_memory(
                            &request,
                            nv_transfer,
                            process.application.resource_user_id,
                            &mut process.memory,
                        )?;
                        if let (Some(backend), Some((descriptor, identity))) =
                            (self.gpu_ioctl_backend.as_mut(), closing_identity)
                        {
                            if client.channel_identity(descriptor).is_none() {
                                backend.close_channel(identity)?;
                            }
                        }
                        response
                    }
                }
                SessionTarget::ParentalSession { initialized } => {
                    crate::parental::dispatch(initialized, &request)?
                }
                SessionTarget::ApmSession => {
                    crate::apm::dispatch_session(&self.performance, &request)?
                }
                SessionTarget::Applet(object) => crate::applet::dispatch_with_display(
                    *object,
                    &mut self.processes.get_mut(&process_id).unwrap().application,
                    &mut self.display,
                    &request,
                )?,
                SessionTarget::Time(object) => {
                    crate::time_service::dispatch(&self.time, *object, &request)?
                }
                SessionTarget::NamedService(name) => {
                    let mut request = request;
                    request.service = name.clone();
                    if name == "aoc:u" {
                        if matches!(request.command_id, 2 | 3) && hipc.handles.process_id.is_none()
                        {
                            return Err(HorizonError::IpcProtocol(
                                "add-on query requires a PID descriptor".into(),
                            ));
                        }
                        self.addons
                            .dispatch(self.processes[&process_id].program_id, &request)?
                    } else {
                        if name == "nifm:u"
                            && request.command_id == 5
                            && hipc.handles.process_id.is_none()
                        {
                            return Err(HorizonError::IpcProtocol(
                                "nifm CreateGeneralService requires a PID descriptor".into(),
                            ));
                        }
                        self.services.call(&self.profile, &request)?
                    }
                }
                SessionTarget::FileSystem(object) => {
                    crate::fsp_object::dispatch(&mut self.fs, object, &request)?
                }
            }
        };
        let dispatched = crate::ipc::finish_dispatch(version, response);
        if self.ipc_diagnostics.len() == 16 {
            self.ipc_diagnostics.pop_front();
        }
        self.ipc_diagnostics
            .push_back((diagnostic.0, diagnostic.1, dispatched.result_code));

        self.apply_ipc_buffer_writes(process_id, &request_bytes, &dispatched.buffer_writes)?;

        let mut move_handles = Vec::new();
        let mut copy_handles = Vec::new();
        let mut domain_objects = Vec::new();
        for object in dispatched.objects {
            if let crate::ipc::ReturnedObject::ReadableEvent(event) = object {
                copy_handles.push(
                    self.process_mut(process_id)?
                        .handles
                        .insert(KernelObject::ReadableEvent(event))?
                        .0,
                );
                continue;
            }
            if let crate::ipc::ReturnedObject::HidSharedMemory { resource_id } = object {
                if resource_id != self.process(process_id)?.application.resource_user_id {
                    return Err(HorizonError::InvalidState(
                        "foreign HID shared memory resource".into(),
                    ));
                }
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    self.hid_shared_memory.entry(resource_id)
                {
                    // HID retains its publication buffer independently of any client handle.
                    entry.insert(crate::shared_memory::SharedMemory::with_guest_permission(
                        0x40000,
                        crate::memory::MemoryPermission::READ,
                    )?);
                }
                let memory = self.hid_shared_memory[&resource_id].clone();
                copy_handles.push(
                    self.process_mut(process_id)?
                        .handles
                        .insert(KernelObject::SharedMemory(memory))?
                        .0,
                );
                continue;
            }
            if object == crate::ipc::ReturnedObject::AddOnListEvent {
                let event_id = self.addons.changed_event.id;
                copy_handles.push(
                    self.process_mut(process_id)?
                        .handles
                        .insert(KernelObject::Event(event_id))?
                        .0,
                );
                continue;
            }
            if object == crate::ipc::ReturnedObject::AppletMessageEvent {
                let event_id = self.process(process_id)?.application.notification_event.id;
                copy_handles.push(
                    self.process_mut(process_id)?
                        .handles
                        .insert(KernelObject::Event(event_id))?
                        .0,
                );
                continue;
            }
            if domain_request.is_some() {
                let target = match object {
                    object @ (crate::ipc::ReturnedObject::FileSystem { .. }
                    | crate::ipc::ReturnedObject::File { .. }
                    | crate::ipc::ReturnedObject::Directory { .. }
                    | crate::ipc::ReturnedObject::Storage { .. }) => {
                        SessionTarget::FileSystem(self.returned_fs_object(object)?)
                    }
                    crate::ipc::ReturnedObject::Nifm(object) => SessionTarget::Nifm(object),
                    crate::ipc::ReturnedObject::Hid(object) => SessionTarget::Hid(object),
                    crate::ipc::ReturnedObject::Vi(object) => SessionTarget::Vi(object),
                    crate::ipc::ReturnedObject::Time(object) => SessionTarget::Time(object),
                    crate::ipc::ReturnedObject::ParentalSession { initialized } => {
                        SessionTarget::ParentalSession { initialized }
                    }
                    crate::ipc::ReturnedObject::Applet(object) => SessionTarget::Applet(object),
                    crate::ipc::ReturnedObject::ApmSession => SessionTarget::ApmSession,
                    crate::ipc::ReturnedObject::Logger { process_id } => {
                        SessionTarget::Logger(crate::logging::Logger {
                            process_id,
                            packet_count: 0,
                            byte_count: 0,
                        })
                    }
                    crate::ipc::ReturnedObject::NamedService(name)
                        if self.services.contains(&name) =>
                    {
                        if name == "nvdrv" {
                            SessionTarget::NvDriver(Default::default())
                        } else if name == "bsd:u" {
                            SessionTarget::Bsd(Default::default())
                        } else {
                            SessionTarget::NamedService(name)
                        }
                    }
                    unsupported => {
                        return Err(HorizonError::InvalidState(format!(
                            "returned domain object type is not implemented: {unsupported:?}"
                        )));
                    }
                };
                domain_objects.push(self.domains.get_mut(&session_id).unwrap().insert(target)?);
            } else {
                move_handles.push(self.materialize_returned_object(process_id, object)?.0);
            }
        }
        let cmif_response = crate::cmif::encode_response(
            crate::cmif::CmifOutHeader {
                version: dispatched.version,
                result: dispatched.result_code,
                interface_id: 0,
            },
            &dispatched.payload,
        );
        let cmif_response = if domain_request.is_some() {
            crate::domain::encode(&cmif_response, &domain_objects)
        } else {
            cmif_response
        };
        let response = if move_handles.is_empty() && copy_handles.is_empty() {
            crate::hipc::encode_simple_cmif_response(&cmif_response)
        } else {
            crate::hipc::encode_cmif_response_with_handles(
                &cmif_response,
                &copy_handles,
                &move_handles,
            )?
        };
        self.threads
            .get_mut(&thread_id)
            .ok_or_else(|| {
                HorizonError::InvalidState(format!("thread {thread_id} does not exist"))
            })?
            .ipc_buffer = response;
        Ok(())
    }

    fn materialize_ipc_buffers(
        &self,
        process_id: u64,
        hipc: &crate::hipc::HipcMessage<'_>,
    ) -> HorizonResult<Vec<crate::ipc::IpcBuffer>> {
        use crate::ipc::{IpcBuffer, IpcBufferDirection};

        let process = self.process(process_id)?;
        let mut out = Vec::new();

        let mut push =
            |address: u64, size: u64, direction: IpcBufferDirection| -> HorizonResult<()> {
                let host_size = usize::try_from(size).map_err(|_| {
                    HorizonError::IpcProtocol("HIPC buffer size does not fit host usize".into())
                })?;
                let mut data = Vec::new();
                if matches!(
                    direction,
                    IpcBufferDirection::In | IpcBufferDirection::InOut
                ) && host_size != 0
                {
                    data.resize(host_size, 0);
                    process.memory.read(address, &mut data)?;
                }
                out.push(IpcBuffer {
                    address,
                    capacity: size,
                    direction,
                    data,
                });
                Ok(())
            };

        for descriptor in &hipc.pointers {
            push(
                descriptor.address,
                u64::from(descriptor.size),
                IpcBufferDirection::In,
            )?;
        }
        for descriptor in &hipc.send_buffers {
            push(descriptor.address, descriptor.size, IpcBufferDirection::In)?;
        }
        for descriptor in &hipc.receive_buffers {
            push(descriptor.address, descriptor.size, IpcBufferDirection::Out)?;
        }
        for descriptor in &hipc.exchange_buffers {
            push(
                descriptor.address,
                descriptor.size,
                IpcBufferDirection::InOut,
            )?;
        }
        for descriptor in &hipc.receive_list {
            push(
                descriptor.address,
                u64::from(descriptor.size),
                IpcBufferDirection::Out,
            )?;
        }
        Ok(out)
    }

    fn apply_ipc_buffer_writes(
        &mut self,
        process_id: u64,
        original_request: &[u8],
        writes: &[crate::ipc::IpcBufferWrite],
    ) -> HorizonResult<()> {
        use crate::ipc::IpcBufferDirection;

        if writes.is_empty() {
            return Ok(());
        }
        let hipc = crate::hipc::parse(original_request)?;
        let buffers = self.materialize_ipc_buffers(process_id, &hipc)?;
        for write in writes {
            let buffer = buffers.get(write.buffer_index).ok_or_else(|| {
                HorizonError::IpcProtocol(format!(
                    "service wrote unknown IPC buffer {}",
                    write.buffer_index
                ))
            })?;
            if buffer.direction == IpcBufferDirection::In {
                return Err(HorizonError::IpcProtocol(format!(
                    "service attempted to write input-only IPC buffer {}",
                    write.buffer_index
                )));
            }
            if u64::try_from(write.data.len()).unwrap_or(u64::MAX) > buffer.capacity {
                return Err(HorizonError::IpcProtocol(format!(
                    "service output ({} bytes) exceeds IPC buffer {} capacity ({})",
                    write.data.len(),
                    write.buffer_index,
                    buffer.capacity
                )));
            }
            self.process_mut(process_id)?
                .memory
                .write(buffer.address, &write.data)?;
        }
        Ok(())
    }

    pub fn thread_mut(&mut self, id: u64) -> HorizonResult<&mut HorizonThread> {
        self.threads
            .get_mut(&id)
            .ok_or_else(|| HorizonError::InvalidState(format!("thread {id} does not exist")))
    }

    pub fn ready_threads(&self) -> impl Iterator<Item = &HorizonThread> {
        self.threads
            .values()
            .filter(|t| t.state == ThreadState::Ready)
    }

    fn resolve_session(&self, client_id: u64) -> HorizonResult<u64> {
        match self.sessions.get(&client_id).map(|session| &session.target) {
            Some(SessionTarget::Alias(server_id)) => Ok(*server_id),
            Some(SessionTarget::Closed) | None => {
                Err(HorizonError::InvalidState("session is closed".into()))
            }
            Some(_) => Ok(client_id),
        }
    }

    fn clone_session(
        &mut self,
        process_id: u64,
        client_id: u64,
    ) -> HorizonResult<crate::handle::Handle> {
        let server_id = self.resolve_session(client_id)?;
        let first_clone = server_id == client_id;
        let new_server_id = if first_clone {
            self.next_session_id
        } else {
            server_id
        };
        let clone_id = self
            .next_session_id
            .checked_add(u64::from(first_clone))
            .ok_or_else(|| HorizonError::InvalidState("session IDs exhausted".into()))?;
        let next_id = clone_id
            .checked_add(1)
            .ok_or_else(|| HorizonError::InvalidState("session IDs exhausted".into()))?;
        // Reserve the native handle before moving any server state. Allocation
        // failure must leave the original object/domain and ID cursor unchanged.
        let handle = self
            .process_mut(process_id)?
            .handles
            .insert(KernelObject::Session(clone_id))?;
        if first_clone {
            let target = std::mem::replace(
                &mut self.sessions.get_mut(&client_id).unwrap().target,
                SessionTarget::Alias(new_server_id),
            );
            self.sessions.insert(
                new_server_id,
                HorizonSession {
                    id: new_server_id,
                    target,
                },
            );
            if let Some(domain) = self.domains.remove(&client_id) {
                self.domains.insert(new_server_id, domain);
            }
        }
        self.sessions.insert(
            clone_id,
            HorizonSession {
                id: clone_id,
                target: SessionTarget::Alias(new_server_id),
            },
        );
        self.next_session_id = next_id;
        Ok(handle)
    }

    fn release_session(&mut self, client_id: u64) -> HorizonResult<()> {
        let server_id = self.resolve_session(client_id)?;
        let last_endpoint = server_id == client_id
            || !self.sessions.values().any(|session| {
                session.id != client_id
                    && matches!(session.target, SessionTarget::Alias(id) if id == server_id)
            });
        if last_endpoint {
            if let (
                Some(backend),
                Some(HorizonSession {
                    target: SessionTarget::NvDriver(client),
                    ..
                }),
            ) = (
                self.gpu_ioctl_backend.as_mut(),
                self.sessions.get(&server_id),
            ) {
                for identity in client.channel_identities() {
                    backend.close_channel(identity)?;
                }
            }
        }
        self.sessions.get_mut(&client_id).unwrap().target = SessionTarget::Closed;
        if server_id == client_id {
            self.domains.remove(&client_id);
        } else if !self
            .sessions
            .values()
            .any(|session| matches!(session.target, SessionTarget::Alias(id) if id == server_id))
        {
            // Canonical server objects have no client handle. Destroy them only
            // after the last live endpoint releases its reference.
            self.sessions.remove(&server_id);
            self.domains.remove(&server_id);
        }
        self.collect_transfer_reservations();
        Ok(())
    }

    /// Transfer references held by services/domains outlive client copy handles.
    fn collect_transfer_reservations(&mut self) {
        let mut objects: Vec<_> = self
            .processes
            .values()
            .flat_map(|process| process.handles.objects())
            .cloned()
            .collect();
        for target in self.sessions.values().map(|session| &session.target).chain(
            self.domains
                .values()
                .flat_map(|domain| domain.objects.values()),
        ) {
            if let SessionTarget::NvDriver(client) = target {
                objects.extend(client.transfer.iter().cloned());
            }
            if let SessionTarget::Bsd(client) = target {
                objects.extend(
                    client
                        .registration
                        .iter()
                        .map(|registration| registration.transfer.clone()),
                );
            }
        }
        for process in self.processes.values_mut() {
            let retained: Vec<_> = objects
                .iter()
                .filter_map(|object| match object {
                    KernelObject::TransferMemory { owner, base, size } if *owner == process.id => {
                        Some((*base, *size))
                    }
                    _ => None,
                })
                .collect();
            process.memory.release_unreferenced_transfers(&retained);
        }
    }

    pub fn create_transfer_memory(
        &mut self,
        process_id: u64,
        base: u64,
        size: u64,
        permission: crate::memory::MemoryPermission,
    ) -> HorizonResult<crate::handle::Handle> {
        // Reserve a handle first so allocation failure cannot leave guest pages locked.
        let handle =
            self.process_mut(process_id)?
                .handles
                .insert(KernelObject::TransferMemory {
                    owner: process_id,
                    base,
                    size,
                })?;
        if let Err(error) = self
            .process_mut(process_id)?
            .memory
            .reserve_transfer(base, size, permission)
        {
            self.process_mut(process_id)?.handles.close(handle)?;
            return Err(error);
        }
        Ok(handle)
    }

    pub fn close_handle(
        &mut self,
        process_id: u64,
        handle: crate::handle::Handle,
    ) -> HorizonResult<()> {
        let object = self.process_mut(process_id)?.handles.close(handle)?;
        if let KernelObject::Session(id) = object {
            if self
                .sessions
                .get(&id)
                .is_some_and(|session| !matches!(session.target, SessionTarget::Closed))
            {
                self.release_session(id)?;
            }
            self.sessions.remove(&id);
        }
        self.collect_transfer_reservations();
        Ok(())
    }

    pub fn event(&self, id: u64) -> HorizonResult<&crate::event::KernelEvent> {
        if id == self.addons.changed_event.id {
            return Ok(&self.addons.changed_event);
        }
        self.processes
            .values()
            .map(|process| &process.application.notification_event)
            .find(|event| event.id == id)
            .ok_or_else(|| HorizonError::InvalidState("event does not exist".into()))
    }

    fn object_is_signaled(&self, object: &KernelObject) -> HorizonResult<bool> {
        match object {
            KernelObject::Event(id) => Ok(self.event(*id)?.signaled),
            KernelObject::ReadableEvent(event) => Ok(event.is_signaled()),
            KernelObject::Thread(id) => Ok(self.thread(*id)?.state == ThreadState::Exited),
            KernelObject::Process(id) => Ok(self.process(*id)?.state == ProcessState::Exited),
            _ => Err(HorizonError::InvalidState("object is not waitable".into())),
        }
    }

    /// Wait on resolved object identities, not handle slots that may later be reused.
    pub fn wait_synchronization(
        &mut self,
        process_id: u64,
        thread_id: u64,
        handles: &[crate::handle::Handle],
        timeout_ns: i64,
    ) -> HorizonResult<(u32, u32)> {
        if self.thread(thread_id)?.process_id != process_id || handles.len() > 64 {
            return Err(HorizonError::InvalidState(
                "invalid synchronization caller/count".into(),
            ));
        }
        let mut objects = Vec::with_capacity(handles.len());
        for handle in handles {
            let object = self.process(process_id)?.handles.get(*handle)?.clone();
            // Validate the entire array before choosing a signaled entry.
            self.object_is_signaled(&object)?;
            objects.push(object);
        }
        for (index, object) in objects.iter().enumerate() {
            if self.object_is_signaled(object)? {
                return Ok((0, index as u32));
            }
        }
        if timeout_ns == 0 {
            return Ok((0xea01, u32::MAX));
        }
        let deadline_ns = (timeout_ns > 0).then(|| {
            self.time
                .steady_clock()
                .time_point_ns
                .saturating_add(timeout_ns as u64)
        });
        self.synchronization_waits.insert(
            thread_id,
            SynchronizationWait {
                objects,
                deadline_ns,
            },
        );
        self.thread_mut(thread_id)?.state = ThreadState::Waiting;
        // Registers are provisional while suspended; poll_synchronization writes
        // the final result before making the thread runnable again.
        Ok((0, u32::MAX))
    }

    pub fn poll_synchronization(&mut self) -> HorizonResult<()> {
        let now = self.time.steady_clock().time_point_ns;
        self.poll_condition_deadlines(now)?;
        let mut completed = Vec::new();
        for (&thread_id, wait) in &self.synchronization_waits {
            if self.thread(thread_id)?.state == ThreadState::Exited {
                completed.push((thread_id, None));
                continue;
            }
            let mut result = None;
            for (index, object) in wait.objects.iter().enumerate() {
                if self.object_is_signaled(object)? {
                    result = Some((0, index as u32));
                    break;
                }
            }
            if result.is_none() && wait.deadline_ns.is_some_and(|deadline| now >= deadline) {
                result = Some((0xea01, u32::MAX));
            }
            if result.is_some() {
                completed.push((thread_id, result));
            }
        }
        for (thread_id, result) in completed {
            self.synchronization_waits.remove(&thread_id);
            if let Some((code, index)) = result {
                let thread = self.thread_mut(thread_id)?;
                thread.context.x[0] = code as u64;
                thread.context.x[1] = index as u64;
                thread.state = ThreadState::Ready;
                let priority = thread.priority;
                self.scheduler.enqueue_with_priority(thread_id, priority);
            }
        }
        Ok(())
    }

    fn validate_applet_process_handle(
        &self,
        process_id: u64,
        message: &crate::hipc::HipcMessage<'_>,
    ) -> HorizonResult<()> {
        if message.handles.process_id.is_none()
            || message.handles.copy_handles.len() != 1
            || !message.handles.move_handles.is_empty()
        {
            return Err(HorizonError::IpcProtocol(
                "OpenApplicationProxy requires a PID and one process copy-handle".into(),
            ));
        }
        // Horizon replaces the sender PID and resolves pseudo-handles in transport.
        // Never trust the PID word supplied by guest code as its own identity.
        let handle = crate::handle::Handle(message.handles.copy_handles[0]);
        if handle.0 != 0xffff8001
            && self.process(process_id)?.handles.get(handle)? != &KernelObject::Process(process_id)
        {
            return Err(HorizonError::InvalidHandle(handle.0));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn primary_npad_poll_uses_injected_time_and_coalesces_missed_intervals() {
        use super::*;
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        struct Clock(Arc<AtomicU64>);
        impl crate::time::TimeSource for Clock {
            fn monotonic_ns(&self) -> u64 {
                self.0.load(Ordering::Relaxed)
            }
            fn unix_seconds(&self) -> i64 {
                0
            }
        }
        let clock = Arc::new(AtomicU64::new(0));
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        kernel.time = crate::time::TimeManager::new(Box::new(Clock(clock.clone())));
        let pid = kernel.create_process(1, "input-clock");
        let identity = kernel.process(pid).unwrap().application.resource_user_id;
        assert!(!kernel.poll_primary_npad(pid).unwrap());
        assert!(kernel.hid_primary_last_sample_ns.is_empty());
        let memory = crate::shared_memory::SharedMemory::new(0x40000).unwrap();
        kernel.hid_shared_memory.insert(identity, memory.clone());
        assert!(kernel.poll_primary_npad(pid).unwrap());
        assert!(!kernel.poll_primary_npad(pid).unwrap());
        clock.store(7_999_999, Ordering::Relaxed);
        assert!(!kernel.poll_primary_npad(pid).unwrap());
        clock.store(8_000_000, Ordering::Relaxed);
        assert!(kernel.poll_primary_npad(pid).unwrap());
        clock.store(1_000_000_000, Ordering::Relaxed);
        assert!(kernel.poll_primary_npad(pid).unwrap());
        assert_eq!(memory.revision(), 3);
        let mut header = [0; 0x20];
        memory.read(0x9a28, &mut header).unwrap();
        assert_eq!(u64::from_le_bytes(header[24..32].try_into().unwrap()), 3);
    }

    #[test]
    fn primary_npad_publication_reaches_retained_application_memory() {
        use super::*;
        use crate::ipc::{IpcBuffer, IpcBufferDirection, ServiceRequest};
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "input-reader");
        let identity = kernel.process(pid).unwrap().application.resource_user_id;
        assert!(!kernel.publish_primary_npad(pid).unwrap());
        let memory = crate::shared_memory::SharedMemory::with_guest_permission(
            0x40000,
            crate::memory::MemoryPermission::READ,
        )
        .unwrap();
        kernel.hid_shared_memory.insert(identity, memory.clone());
        let guest_base = 0x0040_0000;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map_shared(guest_base, &memory, crate::memory::MemoryPermission::READ)
            .unwrap();
        let mut request = ServiceRequest {
            service: "hid".into(),
            command_id: 103,
            raw_payload: identity.to_le_bytes().to_vec(),
            values: Vec::new(),
            buffers: Vec::new(),
        };
        kernel.services.call(&kernel.profile, &request).unwrap();
        request.command_id = 100;
        request.raw_payload = [1u64.to_le_bytes(), identity.to_le_bytes()].concat();
        kernel.services.call(&kernel.profile, &request).unwrap();
        request.command_id = 102;
        request.raw_payload = identity.to_le_bytes().to_vec();
        request.buffers = vec![IpcBuffer {
            address: 0,
            capacity: 4,
            direction: IpcBufferDirection::In,
            data: 0u32.to_le_bytes().to_vec(),
        }];
        kernel.services.call(&kernel.profile, &request).unwrap();
        kernel.hid.set_primary_npad(crate::hid::NpadState {
            connected: true,
            buttons: 3,
            left_stick: [-123, 456],
            ..Default::default()
        });
        assert!(kernel.publish_primary_npad(pid).unwrap());
        assert_eq!(memory.revision(), 1);
        let mut style = [0; 4];
        memory.read(0x9a00, &mut style).unwrap();
        assert_eq!(u32::from_le_bytes(style), 1);
        let mut sample = [0; 0x28];
        memory.read(0x9a28 + 0x20 + 8, &mut sample).unwrap();
        assert_eq!(sample, kernel.hid.primary_npad.encode_common_sample(1));
        let mut guest_sample = [0; 0x28];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(guest_base + 0x9a28 + 0x20 + 8, &mut guest_sample)
            .unwrap();
        assert_eq!(guest_sample, sample);
        assert!(
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(guest_base + 0x9a00, &[0xff; 4])
                .is_err()
        );
        assert_eq!(memory.revision(), 1);
        // Removing player zero disconnects published controls but preserves
        // consecutive history rather than leaving an old button held forever.
        request.buffers[0].data = 1u32.to_le_bytes().to_vec();
        kernel.services.call(&kernel.profile, &request).unwrap();
        kernel.publish_primary_npad(pid).unwrap();
        assert_eq!(memory.revision(), 2);
        memory.read(0x9a00, &mut style).unwrap();
        assert_eq!(u32::from_le_bytes(style), 0);
        memory.read(0x9a28 + 0x20 + 0x30 + 8, &mut sample).unwrap();
        assert_eq!(
            sample,
            crate::hid::NpadState::default().encode_common_sample(2)
        );
        let foreign = kernel.create_process(2, "other-input-reader");
        assert!(!kernel.publish_primary_npad(foreign).unwrap());
    }

    #[test]
    fn gpu_channel_access_resolves_aliases_and_enforces_process_ownership() {
        use crate::{
            ipc::{IpcBuffer, IpcBufferDirection, ServiceRequest},
            memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        };
        let mut kernel = super::HorizonKernel::new(crate::HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "GPU owner");
        for address in [0x4000, 0x8000] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .map(
                    address,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
        }
        let transfer = kernel
            .create_transfer_memory(pid, 0x4000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let transfer = kernel
            .process(pid)
            .unwrap()
            .handles
            .get(transfer)
            .unwrap()
            .clone();
        let root = kernel.connect_service(pid, "nvdrv").unwrap();
        let owner = kernel.process(pid).unwrap().application.resource_user_id;
        let crate::handle::KernelObject::Session(id) =
            *kernel.process(pid).unwrap().handles.get(root).unwrap()
        else {
            unreachable!()
        };
        let server = kernel.resolve_session(id).unwrap();
        let super::SessionTarget::NvDriver(client) =
            &mut kernel.sessions.get_mut(&server).unwrap().target
        else {
            unreachable!()
        };
        let mut request = ServiceRequest {
            service: "nvdrv".into(),
            command_id: 3,
            values: vec![],
            raw_payload: vec![],
            buffers: vec![],
        };
        client.dispatch(&request, Some(transfer), owner).unwrap();
        let mut descriptors = Vec::new();
        for path in [
            b"/dev/nvmap".as_slice(),
            b"/dev/nvhost-as-gpu",
            b"/dev/nvhost-gpu",
        ] {
            request.command_id = 0;
            request.buffers = vec![IpcBuffer {
                address: 0x8000,
                capacity: path.len() as u64,
                direction: IpcBufferDirection::In,
                data: path.to_vec(),
            }];
            let reply = client.dispatch(&request, None, owner).unwrap();
            descriptors.push(u32::from_le_bytes(
                reply.raw_payload[..4].try_into().unwrap(),
            ));
        }
        let (map, space, channel) = (descriptors[0], descriptors[1], descriptors[2]);
        assert!(
            kernel
                .with_gpu_channel(pid, root, channel, |_, _, _| panic!(
                    "unbound channel must reject"
                ))
                .is_err()
        );
        let super::SessionTarget::NvDriver(client) =
            &mut kernel.sessions.get_mut(&server).unwrap().target
        else {
            unreachable!()
        };
        request.command_id = 1;
        for (fd, ioctl, data) in [
            (space, 0x40284109u32, vec![0; 40]),
            (space, 0x40044101, channel.to_le_bytes().to_vec()),
            (channel, 0x40044801, map.to_le_bytes().to_vec()),
        ] {
            request.raw_payload = [fd.to_le_bytes(), ioctl.to_le_bytes()].concat();
            request.buffers = vec![IpcBuffer {
                address: 0x8000,
                capacity: data.len() as u64,
                direction: IpcBufferDirection::In,
                data,
            }];
            assert_eq!(
                client.dispatch(&request, None, owner).unwrap().raw_payload,
                0u32.to_le_bytes()
            );
        }
        let identity = kernel
            .with_gpu_channel(pid, root, channel, |identity, memory, gpu| {
                assert!(gpu.configuration().is_some());
                memory.write(0x8000, &[1, 2, 3, 4]).unwrap();
                identity
            })
            .unwrap();
        let clone = kernel.clone_session(pid, id).unwrap();
        kernel.close_handle(pid, root).unwrap();
        let retained = kernel
            .with_gpu_channel(pid, clone, channel, |identity, memory, gpu| {
                assert!(gpu.configuration().is_some());
                let mut bytes = [0; 4];
                memory.read(0x8000, &mut bytes).unwrap();
                assert_eq!(bytes, [1, 2, 3, 4]);
                identity
            })
            .unwrap();
        assert_eq!(identity, retained);
        assert!(
            kernel
                .with_gpu_channel(pid, clone, map, |_, _, _| panic!("wrong device kind"))
                .is_err()
        );
        let other = kernel.create_process(2, "foreign GPU caller");
        let object = kernel
            .process(pid)
            .unwrap()
            .handles
            .get(clone)
            .unwrap()
            .clone();
        let foreign_handle = kernel
            .process_mut(other)
            .unwrap()
            .handles
            .insert(object)
            .unwrap();
        assert!(
            kernel
                .with_gpu_channel(other, foreign_handle, channel, |_, _, _| panic!(
                    "foreign owner"
                ))
                .is_err()
        );
        kernel.close_handle(pid, clone).unwrap();
        assert!(
            kernel
                .with_gpu_channel(pid, clone, channel, |_, _, _| panic!("closed handle"))
                .is_err()
        );
    }

    use super::*;
    use crate::{
        memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        svc::{SvcArguments, dispatch},
    };

    #[test]
    fn used_memory_info_resolves_process_handles_and_rejects_wrong_subtypes() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        let (tid, thread_handle) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let process_handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Process(pid))
            .unwrap();
        let mut args = SvcArguments::default();
        args.x[1] = 6;
        args.x[2] = 0xffff8001;
        assert!(dispatch(&mut kernel, pid, tid, 0x29, args).is_err());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .set_memory_limit(2 * PAGE_SIZE)
            .unwrap();
        assert_eq!(
            dispatch(&mut kernel, pid, tid, 0x29, args).unwrap().x[1],
            2 * PAGE_SIZE
        );
        args.x[1] = 7;
        for handle in [0xffff8001, process_handle.0] {
            args.x[2] = handle as u64;
            assert_eq!(
                dispatch(&mut kernel, pid, tid, 0x29, args).unwrap().x[1],
                PAGE_SIZE
            );
        }
        args.x[2] = thread_handle.0 as u64;
        assert!(dispatch(&mut kernel, pid, tid, 0x29, args).is_err());
        args.x[2] = process_handle.0 as u64;
        args.x[3] = 1;
        assert!(dispatch(&mut kernel, pid, tid, 0x29, args).is_err());
    }

    #[test]
    fn clone_manager_shares_mutable_object_and_last_close_reclaims_server() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let original = kernel
            .materialize_returned_object(
                pid,
                crate::ipc::ReturnedObject::ParentalSession { initialized: false },
            )
            .unwrap();
        let KernelObject::Session(client_id) =
            kernel.process(pid).unwrap().handles.get(original).unwrap()
        else {
            panic!("session");
        };
        let client_id = *client_id;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&5u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&2u32.to_le_bytes());
        kernel.thread_mut(tid).unwrap().ipc_buffer = request.to_vec();
        kernel.send_sync_request(pid, tid, original).unwrap();
        let response = crate::hipc::parse(&kernel.thread(tid).unwrap().ipc_buffer).unwrap();
        assert_eq!(response.handles.move_handles.len(), 1);
        let clone = crate::handle::Handle(response.handles.move_handles[0]);
        let server_id = kernel.resolve_session(client_id).unwrap();
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[24..28].copy_from_slice(&1u32.to_le_bytes());
        kernel.thread_mut(tid).unwrap().ipc_buffer = request.to_vec();
        kernel.send_sync_request(pid, tid, original).unwrap();
        assert!(matches!(
            kernel.sessions[&server_id].target,
            SessionTarget::ParentalSession { initialized: true }
        ));
        kernel.close_handle(pid, original).unwrap();
        kernel.thread_mut(tid).unwrap().ipc_buffer = request.to_vec();
        kernel.send_sync_request(pid, tid, clone).unwrap();
        kernel.close_handle(pid, clone).unwrap();
        assert!(!kernel.sessions.contains_key(&server_id));
    }

    #[test]
    fn domain_clones_share_ids_and_handle_failure_does_not_move_original() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let original = kernel.connect_service(pid, "apm").unwrap();
        let KernelObject::Session(client_id) =
            kernel.process(pid).unwrap().handles.get(original).unwrap()
        else {
            panic!("session");
        };
        let client_id = *client_id;
        kernel.sessions.get_mut(&client_id).unwrap().target = SessionTarget::Domain;
        kernel.domains.insert(
            client_id,
            crate::domain::Domain::new(SessionTarget::ApmSession),
        );
        let clone = kernel.clone_session(pid, client_id).unwrap();
        let server_id = kernel.resolve_session(client_id).unwrap();
        assert_eq!(kernel.domains[&server_id].objects.len(), 1);
        assert!(!kernel.domains.contains_key(&client_id));
        kernel
            .domains
            .get_mut(&server_id)
            .unwrap()
            .insert(SessionTarget::ApmSession)
            .unwrap();
        kernel.release_session(client_id).unwrap();
        assert_eq!(kernel.domains[&server_id].objects.len(), 2);
        kernel.close_handle(pid, original).unwrap();
        kernel.close_handle(pid, clone).unwrap();
        assert!(!kernel.domains.contains_key(&server_id));

        let pid = kernel.create_process(2, "bounded");
        kernel.process_mut(pid).unwrap().handles = crate::handle::HandleTable::new(1);
        let original = kernel.connect_service(pid, "apm").unwrap();
        let KernelObject::Session(id) = kernel.process(pid).unwrap().handles.get(original).unwrap()
        else {
            panic!("session");
        };
        let id = *id;
        let cursor = kernel.next_session_id;
        assert!(kernel.clone_session(pid, id).is_err());
        assert_eq!(kernel.next_session_id, cursor);
        assert_eq!(kernel.resolve_session(id).unwrap(), id);
        assert!(matches!(
            kernel.sessions[&id].target,
            SessionTarget::NamedService(_)
        ));
    }

    #[test]
    fn parental_factory_requires_pid_and_returns_a_mutable_typed_session() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "pctl:a").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&10u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&1u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
        request.fill(0);
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&(0x80000000u32 | 10).to_le_bytes());
        request[8..12].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&1u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(message.handles.move_handles.len(), 1);
        let child = crate::handle::Handle(message.handles.move_handles[0]);
        let KernelObject::Session(id) = kernel.process(pid).unwrap().handles.get(child).unwrap()
        else {
            panic!("expected parental session");
        };
        let id = *id;
        assert!(matches!(
            kernel.sessions[&id].target,
            SessionTarget::ParentalSession { initialized: false }
        ));
        request.fill(0);
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&1u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, child).unwrap();
        assert!(matches!(
            kernel.sessions[&id].target,
            SessionTarget::ParentalSession { initialized: true }
        ));
    }

    #[test]
    fn addon_discovery_event_is_a_copy_handle_and_starts_unsignaled() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "aoc:u").unwrap();
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&8u32.to_le_bytes());
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(message.handles.copy_handles.len(), 1);
        assert!(message.handles.move_handles.is_empty());
        let handle = crate::handle::Handle(message.handles.copy_handles[0]);
        let KernelObject::Event(id) = kernel.process(pid).unwrap().handles.get(handle).unwrap()
        else {
            panic!("expected catalog event");
        };
        let id = *id;
        assert!(!kernel.event(id).unwrap().signaled);
        assert_eq!(
            kernel.wait_synchronization(pid, tid, &[handle], 0).unwrap(),
            (0xea01, u32::MAX)
        );
        kernel.addons.register_validated(1, 2);
        assert!(kernel.event(id).unwrap().signaled);
        assert_eq!(
            kernel.wait_synchronization(pid, tid, &[handle], 0).unwrap(),
            (0, 0)
        );
    }

    #[test]
    fn synchronization_validates_all_handles_and_wakes_on_signal_or_deadline() {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        struct Clock(Arc<AtomicU64>);
        impl crate::time::TimeSource for Clock {
            fn monotonic_ns(&self) -> u64 {
                self.0.load(Ordering::Relaxed)
            }
            fn unix_seconds(&self) -> i64 {
                0
            }
        }
        let clock = Arc::new(AtomicU64::new(100));
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        kernel.time = TimeManager::new(Box::new(Clock(clock.clone())));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let event_id = kernel
            .process(pid)
            .unwrap()
            .application
            .notification_event
            .id;
        let handle = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(KernelObject::Event(event_id))
            .unwrap();
        assert_eq!(
            kernel
                .wait_synchronization(pid, tid, &[handle], -1)
                .unwrap(),
            (0, 0)
        );
        assert!(
            kernel
                .wait_synchronization(pid, tid, &[handle, crate::handle::Handle(0)], -1)
                .is_err()
        );
        kernel
            .process_mut(pid)
            .unwrap()
            .application
            .notification_event
            .clear();
        assert_eq!(
            kernel.wait_synchronization(pid, tid, &[handle], 0).unwrap(),
            (0xea01, u32::MAX)
        );
        kernel
            .wait_synchronization(pid, tid, &[handle], 10)
            .unwrap();
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Waiting);
        clock.store(109, Ordering::Relaxed);
        kernel.poll_synchronization().unwrap();
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Waiting);
        clock.store(110, Ordering::Relaxed);
        kernel.poll_synchronization().unwrap();
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Ready);
        assert_eq!(kernel.thread(tid).unwrap().context.x[0], 0xea01);
        kernel
            .wait_synchronization(pid, tid, &[handle], -1)
            .unwrap();
        // Pending waits retain object identity even if the caller closes its handle.
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .application
            .notification_event
            .signal();
        kernel.poll_synchronization().unwrap();
        assert_eq!(kernel.thread(tid).unwrap().context.x[0], 0);
        assert_eq!(kernel.thread(tid).unwrap().context.x[1], 0);
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Ready);
    }

    #[test]
    fn applet_events_are_native_copy_handles_even_in_a_domain() {
        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let root = kernel
                .materialize_returned_object(
                    pid,
                    crate::ipc::ReturnedObject::Applet(
                        crate::applet::AppletObject::CommonStateGetter,
                    ),
                )
                .unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0u8; 0x100];
            request[..4].copy_from_slice(&5u32.to_le_bytes());
            request[4..8].copy_from_slice(&8u32.to_le_bytes());
            request[16..20].copy_from_slice(b"SFCI");
            if domain {
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                request.fill(0);
                request[4..8].copy_from_slice(&12u32.to_le_bytes());
                request[16] = 1;
                request[18..20].copy_from_slice(&16u16.to_le_bytes());
                request[20..24].copy_from_slice(&1u32.to_le_bytes());
                request[32..36].copy_from_slice(b"SFCI");
            }
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            for _ in 0..2 {
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                let mut output = [0u8; 0x100];
                kernel
                    .process(pid)
                    .unwrap()
                    .memory
                    .read(tls, &mut output)
                    .unwrap();
                let message = crate::hipc::parse(&output).unwrap();
                assert_eq!(message.handles.copy_handles.len(), 1);
                assert!(message.handles.move_handles.is_empty());
                let handle = crate::handle::Handle(message.handles.copy_handles[0]);
                let KernelObject::Event(id) =
                    kernel.process(pid).unwrap().handles.get(handle).unwrap()
                else {
                    panic!("expected readable event");
                };
                let id = *id;
                assert!(kernel.event(id).unwrap().signaled);
                if domain {
                    assert_eq!(&output[16..20], &0u32.to_le_bytes());
                    assert_eq!(&output[32..36], b"SFCO");
                }
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .handles
                    .close(handle)
                    .unwrap();
                assert!(kernel.event(id).unwrap().signaled);
            }
        }
    }

    #[test]
    fn applet_proxy_validates_process_handle_and_returns_independent_session() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "appletOE").unwrap();
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&(0x80000000u32 | 10).to_le_bytes());
        request[8..12].copy_from_slice(&3u32.to_le_bytes());
        request[20..24].copy_from_slice(&0xffff8001u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        // A session is not a valid process copy-handle. Rejection must preserve TLS.
        request[20..24].copy_from_slice(&root.0.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        assert_eq!(output, request);
        request[20..24].copy_from_slice(&0xffff8001u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(message.handles.move_handles.len(), 1);
        let returned = crate::handle::Handle(message.handles.move_handles[0]);
        let KernelObject::Session(id) = kernel.process(pid).unwrap().handles.get(returned).unwrap()
        else {
            panic!("application proxy must be a session");
        };
        assert!(matches!(
            kernel.sessions[id].target,
            SessionTarget::Applet(crate::applet::AppletObject::ApplicationProxy)
        ));
        assert_ne!(returned, root);
    }

    #[test]
    fn application_termination_result_guest_abi_does_not_exit_or_read_padding() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let other = kernel.create_process(2, "other");
        kernel.start_process(pid).unwrap();
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let target = kernel
            .materialize_returned_object(
                pid,
                crate::ipc::ReturnedObject::Applet(
                    crate::applet::AppletObject::ApplicationFunctions,
                ),
            )
            .unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&9u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&22u32.to_le_bytes());
        request[32..36].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, target).unwrap();
        assert_eq!(
            kernel.process(pid).unwrap().application.terminate_result,
            0xdeadbeef
        );
        assert_eq!(kernel.process(pid).unwrap().state, ProcessState::Running);
        assert_eq!(
            kernel.process(other).unwrap().application.terminate_result,
            0
        );
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(
            crate::cmif::parse_response(message.raw_data)
                .unwrap()
                .header
                .result,
            0
        );
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[32..36].fill(0);
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, target).is_err());
        assert_eq!(
            kernel.process(pid).unwrap().application.terminate_result,
            0xdeadbeef
        );
        kernel.exit_process(pid).unwrap();
        assert_eq!(
            kernel.process(pid).unwrap().application.terminate_result,
            0xdeadbeef
        );
    }

    #[test]
    fn bsd_guest_bootstrap_retains_transfer_until_the_last_monitor_closes() {
        use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let transfer = kernel
            .create_transfer_memory(pid, 0x8000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let client = kernel.connect_service(pid, "bsd:u").unwrap();
        let monitor = kernel.connect_service(pid, "bsd:u").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&(20u32 | 0x80000000).to_le_bytes());
        request[8..12].copy_from_slice(&3u32.to_le_bytes());
        request[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        request[20..24].copy_from_slice(&transfer.0.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[48..52].copy_from_slice(&1u32.to_le_bytes());
        request[88..96].copy_from_slice(&(PAGE_SIZE * 2).to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, client).is_err());
        let KernelObject::Session(id) = *kernel.process(pid).unwrap().handles.get(client).unwrap()
        else {
            panic!("client session");
        };
        assert!(
            matches!(&kernel.sessions[&id].target, SessionTarget::Bsd(client) if client.registration.is_none())
        );
        request[88..96].copy_from_slice(&PAGE_SIZE.to_le_bytes());
        // A missing PID must fail even with an otherwise valid copy handle.
        request[8..12].copy_from_slice(&2u32.to_le_bytes());
        request[12..16].copy_from_slice(&transfer.0.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, client).is_err());
        request[8..12].copy_from_slice(&3u32.to_le_bytes());
        request[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, client).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        let response = crate::cmif::parse_response(message.raw_data).unwrap();
        assert_eq!(&response.payload[..4], &0u32.to_le_bytes());
        assert!(message.handles.move_handles.is_empty());

        request.fill(0);
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&(10u32 | 0x80000000).to_le_bytes());
        request[8..12].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&1u32.to_le_bytes());
        request[48..56].copy_from_slice(&0u64.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, monitor).unwrap();
        kernel.close_handle(pid, transfer).unwrap();
        kernel.close_handle(pid, client).unwrap();
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .transfer_metadata(0x8000),
            Some((MemoryPermission::NONE, 1))
        );
        kernel.close_handle(pid, monitor).unwrap();
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .transfer_metadata(0x8000),
            None
        );
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x8000, &[42])
            .unwrap();
    }

    #[test]
    fn nifm_general_factory_checks_pid_and_returns_native_or_domain_objects() {
        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let root = kernel.connect_service(pid, "nifm:u").unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0u8; 0x100];
            if domain {
                request[..4].copy_from_slice(&5u32.to_le_bytes());
                request[4..8].copy_from_slice(&8u32.to_le_bytes());
                request[16..20].copy_from_slice(b"SFCI");
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
            }
            // Without a PID descriptor, factory failure must not allocate a child.
            request.fill(0);
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            let mut cmif = 16;
            request[4..8].copy_from_slice(&(if domain { 14u32 } else { 10u32 }).to_le_bytes());
            if domain {
                request[16] = 1;
                request[18..20].copy_from_slice(&24u16.to_le_bytes());
                request[20..24].copy_from_slice(&1u32.to_le_bytes());
                cmif = 32;
            }
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 8..cmif + 12].copy_from_slice(&5u32.to_le_bytes());
            let cursor = kernel.next_session_id;
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
            assert_eq!(kernel.next_session_id, cursor);

            request.fill(0);
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            request[4..8].copy_from_slice(
                &((if domain { 16u32 } else { 12u32 }) | 0x80000000).to_le_bytes(),
            );
            request[8..12].copy_from_slice(&1u32.to_le_bytes());
            // The wire PID is not authority; the kernel already knows the caller.
            request[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
            cmif = 32;
            if domain {
                request[32] = 1;
                request[34..36].copy_from_slice(&24u16.to_le_bytes());
                request[36..40].copy_from_slice(&1u32.to_le_bytes());
                cmif = 48;
            }
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 8..cmif + 12].copy_from_slice(&5u32.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            let mut output = [0u8; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            let target = if domain {
                assert!(message.handles.move_handles.is_empty());
                assert_eq!(&output[48..52], &2u32.to_le_bytes());
                let KernelObject::Session(id) =
                    *kernel.process(pid).unwrap().handles.get(root).unwrap()
                else {
                    panic!("root session");
                };
                kernel.domains[&id].objects.get(&2).unwrap()
            } else {
                assert_eq!(message.handles.move_handles.len(), 1);
                let child = crate::handle::Handle(message.handles.move_handles[0]);
                let KernelObject::Session(id) =
                    *kernel.process(pid).unwrap().handles.get(child).unwrap()
                else {
                    panic!("child session");
                };
                &kernel.sessions[&id].target
            };
            assert!(matches!(
                target,
                SessionTarget::Nifm(crate::nifm::NifmObject::General)
            ));
        }
    }

    #[test]
    fn hid_factory_validates_identity_and_returns_native_or_domain_resource() {
        fn factory_request(domain: bool, pid_descriptor: bool, identity: u64) -> [u8; 0x100] {
            let mut request = [0u8; 0x100];
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            let words = if domain { 16u32 } else { 12u32 };
            request[4..8].copy_from_slice(
                &(words | if pid_descriptor { 0x80000000 } else { 0 }).to_le_bytes(),
            );
            let mut cmif = if pid_descriptor {
                request[8..12].copy_from_slice(&1u32.to_le_bytes());
                // A fabricated transmitted PID must not replace caller authority.
                request[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
                32
            } else {
                16
            };
            if domain {
                request[cmif] = 1;
                request[cmif + 2..cmif + 4].copy_from_slice(&24u16.to_le_bytes());
                request[cmif + 4..cmif + 8].copy_from_slice(&1u32.to_le_bytes());
                cmif += 16;
            }
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 16..cmif + 24].copy_from_slice(&identity.to_le_bytes());
            request
        }

        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "hid-caller");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let root = kernel.connect_service(pid, "hid").unwrap();
            if domain {
                let mut control = [0; 0x100];
                control[..4].copy_from_slice(&5u32.to_le_bytes());
                control[4..8].copy_from_slice(&8u32.to_le_bytes());
                control[16..20].copy_from_slice(b"SFCI");
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &control)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
            }
            let identity = kernel.process(pid).unwrap().application.resource_user_id;
            for (pid_descriptor, identity) in [(false, identity), (true, 0), (true, u64::MAX)] {
                let request = factory_request(domain, pid_descriptor, identity);
                let cursor = kernel.next_session_id;
                let handles = kernel.process(pid).unwrap().handles.objects().count();
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
                assert_eq!(kernel.next_session_id, cursor);
                assert_eq!(
                    kernel.process(pid).unwrap().handles.objects().count(),
                    handles
                );
            }
            let mut truncated = factory_request(domain, true, identity);
            if domain {
                truncated[34..36].copy_from_slice(&16u16.to_le_bytes());
            } else {
                truncated[4..8].copy_from_slice(&(8u32 | 0x80000000).to_le_bytes());
            }
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &truncated)
                .unwrap();
            assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
            let request = factory_request(domain, true, identity);
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            let mut reply = [0; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut reply)
                .unwrap();
            let message = crate::hipc::parse(&reply).unwrap();
            let resource_handle = if domain {
                assert!(message.handles.move_handles.is_empty());
                let KernelObject::Session(id) =
                    *kernel.process(pid).unwrap().handles.get(root).unwrap()
                else {
                    panic!("root");
                };
                assert!(
                    matches!(kernel.domains[&id].objects.get(&2), Some(SessionTarget::Hid(crate::hid_service::HidObject::AppletResource { resource_id })) if *resource_id == identity)
                );
                root
            } else {
                assert_eq!(message.handles.move_handles.len(), 1);
                let child = crate::handle::Handle(message.handles.move_handles[0]);
                let KernelObject::Session(id) =
                    *kernel.process(pid).unwrap().handles.get(child).unwrap()
                else {
                    panic!("child");
                };
                assert!(
                    matches!(kernel.sessions[&id].target, SessionTarget::Hid(crate::hid_service::HidObject::AppletResource { resource_id }) if resource_id == identity)
                );
                kernel.close_handle(pid, root).unwrap();
                child
            };
            // Both transports return a native copy handle to the actual buffer.
            let mut child_request = [0; 0x100];
            child_request[..4].copy_from_slice(&4u32.to_le_bytes());
            child_request[4..8].copy_from_slice(&(if domain { 12u32 } else { 8u32 }).to_le_bytes());
            let cmif = if domain {
                child_request[16] = 1;
                child_request[18..20].copy_from_slice(&16u16.to_le_bytes());
                child_request[20..24].copy_from_slice(&2u32.to_le_bytes());
                32
            } else {
                16
            };
            child_request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &child_request)
                .unwrap();
            kernel
                .send_guest_sync_request(pid, tid, resource_handle)
                .unwrap();
            let mut reply = [0; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut reply)
                .unwrap();
            let reply = crate::hipc::parse(&reply).unwrap();
            assert_eq!(reply.handles.copy_handles.len(), 1);
            assert!(reply.handles.move_handles.is_empty());
            let handle = crate::handle::Handle(reply.handles.copy_handles[0]);
            let KernelObject::SharedMemory(memory) =
                kernel.process(pid).unwrap().handles.get(handle).unwrap()
            else {
                panic!("HID returned a non-memory handle");
            };
            assert_eq!(memory.size(), 0x40000);
            assert!(memory.same_object(&kernel.hid_shared_memory[&identity]));
        }
    }

    #[test]
    fn hid_activation_requires_caller_identity_and_pid_descriptor() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "hid");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "hid").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let identity = kernel.process(pid).unwrap().application.resource_user_id;
        let mut style_event = None;
        for (command, (send_pid, aruid, succeeds)) in [0u32, 1, 100, 101, 103, 106]
            .into_iter()
            .flat_map(|command| {
                [
                    (false, identity, false),
                    (true, identity + 1, false),
                    (true, identity, true),
                ]
                .into_iter()
                .map(move |request| (command, request))
            })
        {
            let mut request = [0; 0x100];
            let offset = if matches!(command, 100 | 106) { 8 } else { 0 };
            let raw_words = if matches!(command, 100 | 106) {
                16u32
            } else {
                12u32
            };
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            let cmif = if send_pid {
                request[4..8].copy_from_slice(&(raw_words | 1 << 31).to_le_bytes());
                request[8..12].copy_from_slice(&1u32.to_le_bytes());
                request[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
                32
            } else {
                request[4..8].copy_from_slice(&raw_words.to_le_bytes());
                16
            };
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 8..cmif + 12].copy_from_slice(&command.to_le_bytes());
            if command == 100 {
                request[cmif + 16..cmif + 20].copy_from_slice(&0x1fu32.to_le_bytes());
            }
            request[cmif + 16 + offset..cmif + 24 + offset].copy_from_slice(&aruid.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            let handles_before = kernel.process(pid).unwrap().handles.objects().count();
            assert_eq!(
                kernel.send_guest_sync_request(pid, tid, root).is_ok(),
                succeeds
            );
            if !succeeds {
                assert_eq!(
                    kernel.process(pid).unwrap().handles.objects().count(),
                    handles_before
                );
            } else if command == 106 {
                let mut reply = [0; 0x100];
                kernel
                    .process(pid)
                    .unwrap()
                    .memory
                    .read(tls, &mut reply)
                    .unwrap();
                let response = crate::hipc::parse(&reply).unwrap();
                assert!(response.handles.move_handles.is_empty());
                assert_eq!(response.handles.copy_handles.len(), 1);
                let handle = crate::handle::Handle(response.handles.copy_handles[0]);
                let KernelObject::ReadableEvent(event) =
                    kernel.process(pid).unwrap().handles.get(handle).unwrap()
                else {
                    panic!("HID style update must return a readable event");
                };
                assert!(!event.is_signaled());
                style_event = Some(event.clone());
            }
        }
        let event = style_event.unwrap();
        let request = crate::ipc::ServiceRequest {
            service: "hid".into(),
            command_id: 102,
            values: Vec::new(),
            raw_payload: identity.to_le_bytes().to_vec(),
            buffers: vec![crate::ipc::IpcBuffer {
                address: 0,
                capacity: 4,
                direction: crate::ipc::IpcBufferDirection::In,
                data: 0u32.to_le_bytes().to_vec(),
            }],
        };
        kernel.services.call(&kernel.profile, &request).unwrap();
        kernel.hid_shared_memory.insert(
            identity,
            crate::shared_memory::SharedMemory::new(0x40000).unwrap(),
        );
        kernel.hid.set_primary_npad(crate::hid::NpadState {
            connected: true,
            ..Default::default()
        });
        kernel.publish_primary_npad(pid).unwrap();
        assert!(event.is_signaled());
        event.set_signaled(false);
        kernel.publish_primary_npad(pid).unwrap();
        assert!(!event.is_signaled());
        kernel
            .hid
            .set_primary_npad(crate::hid::NpadState::default());
        kernel.publish_primary_npad(pid).unwrap();
        assert!(event.is_signaled());
    }

    #[test]
    fn managed_display_layer_reply_refers_to_application_owned_layer() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "display");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let handle = kernel
            .materialize_returned_object(
                pid,
                crate::ipc::ReturnedObject::Applet(crate::applet::AppletObject::SelfController),
            )
            .unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&40u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, handle).unwrap();
        let mut reply = [0; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut reply)
            .unwrap();
        let hipc = crate::hipc::parse(&reply).unwrap();
        let cmif = crate::cmif::parse_response(hipc.raw_data).unwrap();
        assert_eq!(cmif.header.result, 0);
        let layer = u64::from_le_bytes(cmif.payload[..8].try_into().unwrap());
        let owner = kernel.process(pid).unwrap().application.resource_user_id;
        assert!(kernel.display.managed_layer(layer, owner).is_some());
        assert!(kernel.display.managed_layer(layer, owner + 1).is_none());
        kernel.close_handle(pid, handle).unwrap();
        assert!(kernel.display.managed_layer(layer, owner).is_some());
        kernel.exit_process(pid).unwrap();
        assert!(kernel.display.managed_layer(layer, owner).is_none());
    }

    #[test]
    fn fsp_access_log_policy_query_uses_native_and_domain_guest_framing() {
        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let root = kernel.connect_service(pid, "fsp-srv").unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0u8; 0x100];
            if domain {
                request[..4].copy_from_slice(&5u32.to_le_bytes());
                request[4..8].copy_from_slice(&8u32.to_le_bytes());
                request[16..20].copy_from_slice(b"SFCI");
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                request.fill(0);
                request[16] = 1;
                request[18..20].copy_from_slice(&16u16.to_le_bytes());
                request[20..24].copy_from_slice(&1u32.to_le_bytes());
            }
            let cmif_offset = if domain { 32 } else { 16 };
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            request[4..8].copy_from_slice(&(if domain { 12u32 } else { 8u32 }).to_le_bytes());
            request[cmif_offset..cmif_offset + 4].copy_from_slice(b"SFCI");
            request[cmif_offset + 8..cmif_offset + 12].copy_from_slice(&1005u32.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            let mut output = [0u8; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            assert!(message.handles.copy_handles.is_empty());
            assert!(message.handles.move_handles.is_empty());
            assert_eq!(&output[cmif_offset..cmif_offset + 4], b"SFCO");
            assert_eq!(
                &output[cmif_offset + 8..cmif_offset + 12],
                &0u32.to_le_bytes()
            );
            assert_eq!(
                &output[cmif_offset + 16..cmif_offset + 20],
                &0u32.to_le_bytes()
            );
            if domain {
                assert_eq!(&output[16..20], &0u32.to_le_bytes());
            }
        }
    }

    #[test]
    fn domain_rom_storage_returns_a_routable_child_without_native_handles() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        kernel.fs.mount_storage(
            "rom-data",
            std::sync::Arc::new(exosphere_vfs::backend::storage::MemoryStorage::new(
                vec![1, 2, 3, 4],
                false,
            )),
        );
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "fsp-srv").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&5u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();

        request.fill(0);
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&12u32.to_le_bytes());
        request[16] = 1;
        request[18..20].copy_from_slice(&16u16.to_le_bytes());
        request[20..24].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&200u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert!(message.handles.move_handles.is_empty());
        assert_eq!(&output[16..20], &1u32.to_le_bytes());
        assert_eq!(&output[48..52], &2u32.to_le_bytes());

        // Address the IStorage child in the same domain, not as a native handle.
        request[20..24].copy_from_slice(&2u32.to_le_bytes());
        request[40..44].copy_from_slice(&4u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        assert_eq!(&output[48..56], &4u64.to_le_bytes());
    }

    #[test]
    fn vi_factory_and_relay_use_typed_guest_move_handles() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "vi:m").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&9u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[24..28].copy_from_slice(&2u32.to_le_bytes());
        request[32..36].copy_from_slice(&1u32.to_le_bytes());
        let mut target = root;
        for kind in [
            crate::vi::ViKind::Application,
            crate::vi::ViKind::BinderRelay,
        ] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, target).unwrap();
            let mut output = [0u8; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            assert_eq!(message.handles.move_handles.len(), 1);
            target = crate::handle::Handle(message.handles.move_handles[0]);
            let KernelObject::Session(id) =
                *kernel.process(pid).unwrap().handles.get(target).unwrap()
            else {
                panic!("VI child must be a session");
            };
            assert!(
                matches!(&kernel.sessions[&id].target, SessionTarget::Vi(object) if object.kind == kind && object.privilege == crate::vi::Privilege::Manager)
            );
            request[24..28].copy_from_slice(&100u32.to_le_bytes());
            request[32..36].fill(0);
        }
        let owner = kernel.process(pid).unwrap().application.resource_user_id;
        let layer = kernel.display.create_managed_layer(owner).unwrap();
        let window = kernel
            .display
            .open_managed_native_window(layer.id, owner)
            .unwrap();
        let parcel = crate::parcel::Parcel::parse(&window).unwrap();
        let producer_id = i32::from_le_bytes(parcel.payload[8..12].try_into().unwrap());
        // The actual relay move handle routes both reference kinds to the same
        // registry object. No PID descriptor is needed for this command.
        request[4..8].copy_from_slice(&11u32.to_le_bytes());
        request[24..28].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(&producer_id.to_le_bytes());
        for (delta, kind, expected) in [
            (1i32, 0u32, (1, 0)),
            (1, 1, (1, 1)),
            (-1, 1, (1, 0)),
            (-1, 0, (0, 0)),
        ] {
            request[36..40].copy_from_slice(&delta.to_le_bytes());
            request[40..44].copy_from_slice(&kind.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, target).unwrap();
            let mut output = [0; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            assert_eq!(&output[24..28], &0u32.to_le_bytes());
            assert_eq!(
                kernel
                    .display
                    .producers
                    .get_mut(producer_id, owner)
                    .unwrap()
                    .reference_counts(),
                expected
            );
        }
    }

    #[test]
    fn layer_scaling_native_and_domain_tls_replies_retain_the_same_layer_policy() {
        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let owner = kernel.process(pid).unwrap().application.resource_user_id;
            let layer = kernel.display.create_managed_layer(owner).unwrap();
            let root = kernel
                .materialize_returned_object(
                    pid,
                    crate::ipc::ReturnedObject::Vi(crate::vi::ViSession::new(
                        crate::vi::ViKind::Application,
                        crate::vi::Privilege::Application,
                    )),
                )
                .unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0; 0x100];
            let cmif = if domain {
                request[..4].copy_from_slice(&5u32.to_le_bytes());
                request[4..8].copy_from_slice(&8u32.to_le_bytes());
                request[16..20].copy_from_slice(b"SFCI");
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                request.fill(0);
                request[4..8].copy_from_slice(&16u32.to_le_bytes());
                request[16] = 1;
                request[18..20].copy_from_slice(&32u16.to_le_bytes());
                request[20..24].copy_from_slice(&1u32.to_le_bytes());
                32
            } else {
                request[4..8].copy_from_slice(&12u32.to_le_bytes());
                16
            };
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 8..cmif + 12].copy_from_slice(&2101u32.to_le_bytes());
            request[cmif + 16..cmif + 20].copy_from_slice(&4u32.to_le_bytes());
            request[cmif + 20..cmif + 24].fill(0xff); // Padding must not truncate/reinterpret the ID.
            request[cmif + 24..cmif + 32].copy_from_slice(&layer.id.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            let mut output = [0; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            assert!(
                message.handles.copy_handles.is_empty() && message.handles.move_handles.is_empty()
            );
            assert_eq!(&output[cmif..cmif + 4], b"SFCO");
            assert_eq!(&output[cmif + 8..cmif + 12], &0u32.to_le_bytes());
            assert_eq!(
                kernel
                    .display
                    .managed_layer(layer.id, owner)
                    .unwrap()
                    .scaling_mode,
                Some(crate::display::LayerScalingMode::PreserveAspectRatio)
            );
            kernel.close_handle(pid, root).unwrap();
            assert_eq!(
                kernel
                    .display
                    .managed_layer(layer.id, owner)
                    .unwrap()
                    .scaling_mode,
                Some(crate::display::LayerScalingMode::PreserveAspectRatio)
            );
        }
    }

    #[test]
    fn producer_events_use_native_copy_handles_in_native_and_domain_replies() {
        for domain in [false, true] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let owner = kernel.process(pid).unwrap().application.resource_user_id;
            let layer = kernel.display.create_managed_layer(owner).unwrap();
            let window = kernel
                .display
                .open_managed_native_window(layer.id, owner)
                .unwrap();
            let parcel = crate::parcel::Parcel::parse(&window).unwrap();
            let producer = i32::from_le_bytes(parcel.payload[8..12].try_into().unwrap());
            let root = kernel
                .materialize_returned_object(
                    pid,
                    crate::ipc::ReturnedObject::Vi(crate::vi::ViSession::new(
                        crate::vi::ViKind::BinderRelay,
                        crate::vi::Privilege::Application,
                    )),
                )
                .unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0; 0x100];
            let cmif = if domain {
                request[..4].copy_from_slice(&5u32.to_le_bytes());
                request[4..8].copy_from_slice(&8u32.to_le_bytes());
                request[16..20].copy_from_slice(b"SFCI");
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                request.fill(0);
                request[4..8].copy_from_slice(&14u32.to_le_bytes());
                request[16] = 1;
                request[18..20].copy_from_slice(&24u16.to_le_bytes());
                request[20..24].copy_from_slice(&1u32.to_le_bytes());
                32
            } else {
                request[4..8].copy_from_slice(&10u32.to_le_bytes());
                16
            };
            request[..4].copy_from_slice(&4u32.to_le_bytes());
            request[cmif..cmif + 4].copy_from_slice(b"SFCI");
            request[cmif + 8..cmif + 12].copy_from_slice(&2u32.to_le_bytes());
            request[cmif + 16..cmif + 20].copy_from_slice(&producer.to_le_bytes());
            request[cmif + 20..cmif + 24].copy_from_slice(&0x0fu32.to_le_bytes());
            let mut handles = Vec::new();
            for _ in 0..2 {
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                let mut output = [0; 0x100];
                kernel
                    .process(pid)
                    .unwrap()
                    .memory
                    .read(tls, &mut output)
                    .unwrap();
                let message = crate::hipc::parse(&output).unwrap();
                assert_eq!(message.handles.copy_handles.len(), 1);
                assert!(message.handles.move_handles.is_empty());
                if domain {
                    assert_eq!(&output[16..20], &0u32.to_le_bytes());
                }
                handles.push(crate::handle::Handle(message.handles.copy_handles[0]));
            }
            let event_object = kernel
                .process(pid)
                .unwrap()
                .handles
                .get(handles[0])
                .unwrap()
                .clone();
            assert_eq!(
                &event_object,
                kernel
                    .process(pid)
                    .unwrap()
                    .handles
                    .get(handles[1])
                    .unwrap()
            );
            assert_eq!(
                kernel
                    .wait_synchronization(pid, tid, &[handles[0]], 0)
                    .unwrap()
                    .0,
                0xea01
            );
            kernel
                .wait_synchronization(pid, tid, &[handles[0]], -1)
                .unwrap();
            kernel.close_handle(pid, handles[0]).unwrap();
            let queue = &mut kernel
                .display
                .producers
                .get_mut(producer, owner)
                .unwrap()
                .queue;
            queue
                .attach(0, crate::shared_memory::SharedMemory::new(4096).unwrap())
                .unwrap();
            queue.connect().unwrap();
            kernel.poll_synchronization().unwrap();
            assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Ready);
            assert_eq!(kernel.thread(tid).unwrap().context.x[0], 0);
            assert_eq!(
                kernel
                    .wait_synchronization(pid, tid, &[handles[1]], 0)
                    .unwrap(),
                (0, 0)
            );
            kernel.display.destroy_layer(layer.id);
            assert!(!kernel.object_is_signaled(&event_object).unwrap());
            kernel.close_handle(pid, handles[1]).unwrap();
        }
    }

    #[test]
    fn extended_clone_accepts_manager_tags_and_shares_canonical_server_state() {
        for message_type in [3u32, 5, 7] {
            let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
            let pid = kernel.create_process(1, "test");
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let root = kernel.connect_service(pid, "nvdrv").unwrap();
            let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
            let mut request = [0u8; 0x100];
            request[..4].copy_from_slice(&message_type.to_le_bytes());
            request[4..8].copy_from_slice(&8u32.to_le_bytes());
            request[16..20].copy_from_slice(b"SFCI");
            request[24..28].copy_from_slice(&4u32.to_le_bytes());
            let cursor = kernel.next_session_id;
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
            assert_eq!(kernel.next_session_id, cursor);
            request[4..8].copy_from_slice(&9u32.to_le_bytes());
            let mut clones = Vec::new();
            for tag in [0u32, 1, u32::MAX] {
                request[32..36].copy_from_slice(&tag.to_le_bytes());
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                let mut output = [0u8; 0x100];
                kernel
                    .process(pid)
                    .unwrap()
                    .memory
                    .read(tls, &mut output)
                    .unwrap();
                let message = crate::hipc::parse(&output).unwrap();
                assert_eq!(message.handles.move_handles.len(), 1);
                assert!(message.handles.copy_handles.is_empty());
                let clone = crate::handle::Handle(message.handles.move_handles[0]);
                assert_ne!(clone, root);
                assert!(!clones.contains(&clone));
                clones.push(clone);
            }
            let KernelObject::Session(root_id) =
                *kernel.process(pid).unwrap().handles.get(root).unwrap()
            else {
                panic!("root must be a session");
            };
            let server = kernel.resolve_session(root_id).unwrap();
            let SessionTarget::NvDriver(client) =
                &mut kernel.sessions.get_mut(&server).unwrap().target
            else {
                panic!("shared server must remain typed");
            };
            client.resource_user_id = Some(42);
            let mut device_request = crate::ipc::ServiceRequest {
                service: "nvdrv".into(),
                command_id: 3,
                values: Vec::new(),
                raw_payload: Vec::new(),
                buffers: Vec::new(),
            };
            client
                .dispatch(
                    &device_request,
                    Some(KernelObject::TransferMemory {
                        owner: pid,
                        base: 0x4000,
                        size: 0x1000,
                    }),
                    42,
                )
                .unwrap();
            device_request.command_id = 0;
            device_request.buffers.push(crate::ipc::IpcBuffer {
                address: 0x1000,
                capacity: b"/dev/nvmap".len() as u64,
                direction: crate::ipc::IpcBufferDirection::In,
                data: b"/dev/nvmap".to_vec(),
            });
            let reply = client.dispatch(&device_request, None, 42).unwrap();
            let descriptor = u32::from_le_bytes(reply.raw_payload[..4].try_into().unwrap());
            kernel.close_handle(pid, root).unwrap();
            for clone in clones {
                let KernelObject::Session(id) =
                    *kernel.process(pid).unwrap().handles.get(clone).unwrap()
                else {
                    panic!("clone must be a session");
                };
                assert_eq!(kernel.resolve_session(id).unwrap(), server);
                assert!(
                    matches!(&kernel.sessions[&server].target, SessionTarget::NvDriver(client)
                        if client.resource_user_id == Some(42)
                        && client.device(descriptor) == Some(crate::nvdrv::NvDeviceKind::Map))
                );
                kernel.close_handle(pid, clone).unwrap();
            }
            assert!(!kernel.sessions.contains_key(&server));
        }
    }

    #[test]
    fn nvdrv_initialization_retains_transfer_memory_across_client_handle_close() {
        use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        let transfer = kernel
            .create_transfer_memory(pid, 0x4000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let root = kernel.connect_service(pid, "nvdrv").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&0x80000009u32.to_le_bytes());
        request[8..12].copy_from_slice(&4u32.to_le_bytes()); // Two HIPC copy handles.
        request[12..16].copy_from_slice(&0xffff8001u32.to_le_bytes());
        request[16..20].copy_from_slice(&transfer.0.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        request[40..44].copy_from_slice(&3u32.to_le_bytes());
        request[48..52].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        let mut invalid = request;
        invalid[48..52].copy_from_slice(&(2 * PAGE_SIZE as u32).to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &invalid)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        let response = crate::cmif::parse_response(message.raw_data).unwrap();
        assert_eq!(response.header.result, 0);
        assert_eq!(&response.payload[..4], &0u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
        let KernelObject::Session(id) = *kernel.process(pid).unwrap().handles.get(root).unwrap()
        else {
            panic!("nvdrv must be a session");
        };
        let clone = kernel.clone_session(pid, id).unwrap();
        kernel.close_handle(pid, transfer).unwrap();
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .transfer_metadata(0x4000),
            Some((MemoryPermission::NONE, 1))
        );
        kernel.close_handle(pid, root).unwrap();
        assert!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(0x4000, &mut [0])
                .is_err()
        );
        kernel.close_handle(pid, clone).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .validate_access(0x4000, 1, MemoryPermission::READ_WRITE)
            .unwrap();
    }

    #[test]
    fn native_nv_query_event_returns_stable_copy_handles_without_signaling() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "nv-event-test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "nvdrv").unwrap();
        let owner = kernel.process(pid).unwrap().application.resource_user_id;
        let KernelObject::Session(session) =
            *kernel.process(pid).unwrap().handles.get(root).unwrap()
        else {
            panic!("expected driver session");
        };
        let server = kernel.resolve_session(session).unwrap();
        let SessionTarget::NvDriver(client) = &mut kernel.sessions.get_mut(&server).unwrap().target
        else {
            panic!("expected driver client");
        };
        let mut request = crate::ipc::ServiceRequest {
            service: "nvdrv".into(),
            command_id: 3,
            values: vec![],
            raw_payload: vec![],
            buffers: vec![],
        };
        // Driver setup is tested separately; this fixture exercises native
        // QueryEvent transport and copied kernel-handle materialization.
        client
            .dispatch(
                &request,
                Some(KernelObject::TransferMemory {
                    owner: pid,
                    base: 0x4000,
                    size: 0x1000,
                }),
                owner,
            )
            .unwrap();
        request.command_id = 0;
        request.buffers.push(crate::ipc::IpcBuffer {
            address: 0,
            capacity: b"/dev/nvhost-gpu".len() as u64,
            direction: crate::ipc::IpcBufferDirection::In,
            data: b"/dev/nvhost-gpu".to_vec(),
        });
        let opened = client.dispatch(&request, None, owner).unwrap();
        let descriptor = u32::from_le_bytes(opened.raw_payload[..4].try_into().unwrap());
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut events = vec![];
        for _ in 0..2 {
            let mut wire = [0u8; 0x100];
            wire[..4].copy_from_slice(&4u32.to_le_bytes());
            wire[4..8].copy_from_slice(&10u32.to_le_bytes());
            wire[16..20].copy_from_slice(b"SFCI");
            wire[24..28].copy_from_slice(&4u32.to_le_bytes());
            wire[32..36].copy_from_slice(&descriptor.to_le_bytes());
            wire[36..40].copy_from_slice(&3u32.to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &wire)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut wire)
                .unwrap();
            let response = crate::hipc::parse(&wire).unwrap();
            assert!(response.handles.move_handles.is_empty());
            assert_eq!(response.handles.copy_handles.len(), 1);
            let handle = crate::handle::Handle(response.handles.copy_handles[0]);
            let KernelObject::ReadableEvent(event) =
                kernel.process(pid).unwrap().handles.get(handle).unwrap()
            else {
                panic!("expected readable event");
            };
            assert!(!event.is_signaled());
            events.push(event.clone());
            kernel.close_handle(pid, handle).unwrap();
        }
        assert_eq!(events[0], events[1]);
    }

    #[test]
    fn native_nvmap_allocation_checks_writeback_before_pinning_and_survives_clones() {
        use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "nvmap-test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "nvdrv").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let process = kernel.process_mut(pid).unwrap();
        process
            .memory
            .map(
                0x4000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        for address in [0x8000, 0x9000, 0xa000, 0xb000] {
            process
                .memory
                .map(
                    address,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    MemoryKind::Heap,
                )
                .unwrap();
        }
        process.memory.write(0x8000, b"/dev/nvmap").unwrap();
        let transfer = kernel
            .create_transfer_memory(pid, 0x4000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let transfer_object = kernel
            .process(pid)
            .unwrap()
            .handles
            .get(transfer)
            .unwrap()
            .clone();
        let owner = kernel.process(pid).unwrap().application.resource_user_id;
        let KernelObject::Session(id) = *kernel.process(pid).unwrap().handles.get(root).unwrap()
        else {
            panic!("expected driver session")
        };
        let server = kernel.resolve_session(id).unwrap();
        let SessionTarget::NvDriver(client) = &mut kernel.sessions.get_mut(&server).unwrap().target
        else {
            panic!("expected driver")
        };
        let initialize = crate::ipc::ServiceRequest {
            service: "nvdrv".into(),
            command_id: 3,
            values: Vec::new(),
            raw_payload: Vec::new(),
            buffers: Vec::new(),
        };
        client
            .dispatch(&initialize, Some(transfer_object), owner)
            .unwrap();
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&(4u32 | (1 << 20)).to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[8..12].copy_from_slice(&(b"/dev/nvmap".len() as u32).to_le_bytes());
        request[12..16].copy_from_slice(&0x8000u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut word = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls + 32, &mut word)
            .unwrap();
        let descriptor = u32::from_le_bytes(word);
        request[..4].copy_from_slice(&(4u32 | (1 << 20) | (1 << 24)).to_le_bytes());
        request[4..8].copy_from_slice(&10u32.to_le_bytes());
        request[8..12].copy_from_slice(&8u32.to_le_bytes());
        request[12..16].copy_from_slice(&0x9000u32.to_le_bytes());
        request[20..24].copy_from_slice(&8u32.to_le_bytes());
        request[24..28].copy_from_slice(&0xa000u32.to_le_bytes());
        request[40..44].copy_from_slice(&1u32.to_le_bytes());
        request[48..52].copy_from_slice(&descriptor.to_le_bytes());
        request[52..56].copy_from_slice(&0xc0080101u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x9000, &[16u32.to_le_bytes(), 0u32.to_le_bytes()].concat())
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0xa004, &mut word)
            .unwrap();
        let handle = u32::from_le_bytes(word);
        request[8..12].copy_from_slice(&32u32.to_le_bytes());
        request[20..24].copy_from_slice(&32u32.to_le_bytes());
        request[52..56].copy_from_slice(&0xc0200104u32.to_le_bytes());
        let mut allocation = [0u8; 32];
        allocation[..4].copy_from_slice(&handle.to_le_bytes());
        allocation[8..12].copy_from_slice(&1u32.to_le_bytes());
        allocation[24..32].copy_from_slice(&0xb000u64.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x9000, &allocation)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .protect(0xa000, PAGE_SIZE, MemoryPermission::READ)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
        let SessionTarget::NvDriver(client) = &kernel.sessions[&server].target else {
            panic!("expected driver")
        };
        assert!(client.map_object(handle).unwrap().backing.is_none());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .protect(0xb000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .protect(0xa000, PAGE_SIZE, MemoryPermission::READ_WRITE)
            .unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let clone = kernel.clone_session(pid, id).unwrap();
        kernel.close_handle(pid, root).unwrap();
        assert!(
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .unmap(0xb000, PAGE_SIZE)
                .is_err()
        );
        kernel.close_handle(pid, clone).unwrap();
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .unmap(0xb000, PAGE_SIZE)
            .unwrap();
    }

    #[test]
    fn transfer_handles_restore_owner_permissions_only_after_last_close() {
        use crate::{
            handle::HandleTable,
            memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        };
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x1000,
                2 * PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Heap,
            )
            .unwrap();
        kernel.process_mut(pid).unwrap().handles = HandleTable::new(0);
        assert!(
            kernel
                .create_transfer_memory(pid, 0x1000, PAGE_SIZE, MemoryPermission::NONE)
                .is_err()
        );
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .transfer_metadata(0x1000),
            None
        );
        kernel.process_mut(pid).unwrap().handles = HandleTable::new(4);
        let first = kernel
            .create_transfer_memory(pid, 0x1000, PAGE_SIZE, MemoryPermission::NONE)
            .unwrap();
        let object = kernel
            .process(pid)
            .unwrap()
            .handles
            .get(first)
            .unwrap()
            .clone();
        let second = kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .insert(object)
            .unwrap();
        kernel.close_handle(pid, first).unwrap();
        assert!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .validate_access(0x1000, 1, MemoryPermission::READ)
                .is_err()
        );
        kernel.close_handle(pid, second).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .validate_access(0x1000, 1, MemoryPermission::READ_WRITE)
            .unwrap();
        assert!(kernel.close_handle(pid, second).is_err());
        assert!(
            kernel
                .create_transfer_memory(pid, 0x1001, PAGE_SIZE, MemoryPermission::NONE)
                .is_err()
        );
        assert_eq!(
            kernel
                .process(pid)
                .unwrap()
                .memory
                .transfer_metadata(0x1000),
            None
        );
    }

    #[test]
    fn time_factory_and_clock_reads_use_real_tls_move_handles() {
        struct FixedTime;
        impl crate::time::TimeSource for FixedTime {
            fn monotonic_ns(&self) -> u64 {
                12_999_999_999
            }
            fn unix_seconds(&self) -> i64 {
                1_700_000_000
            }
        }
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        kernel.time = crate::time::TimeManager::new(Box::new(FixedTime));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "time:u").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        let mut clocks = Vec::new();
        for (command, object) in [
            crate::time_service::TimeObject::UserSystem,
            crate::time_service::TimeObject::NetworkSystem,
            crate::time_service::TimeObject::Steady,
            crate::time_service::TimeObject::TimeZone,
            crate::time_service::TimeObject::LocalSystem,
        ]
        .into_iter()
        .enumerate()
        {
            request[24..28].copy_from_slice(&(command as u32).to_le_bytes());
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel.send_guest_sync_request(pid, tid, root).unwrap();
            let mut output = [0u8; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            assert_eq!(message.handles.move_handles.len(), 1);
            let handle = crate::handle::Handle(message.handles.move_handles[0]);
            let KernelObject::Session(id) =
                kernel.process(pid).unwrap().handles.get(handle).unwrap()
            else {
                panic!("clock must be a session");
            };
            assert!(
                matches!(kernel.sessions[id].target, SessionTarget::Time(actual) if actual == object)
            );
            assert!(!clocks.contains(&handle));
            clocks.push(handle);
        }
        kernel.close_handle(pid, root).unwrap();
        request[24..28].copy_from_slice(&0u32.to_le_bytes());
        for (index, expected) in [(0, 1_700_000_000i64), (2, 12), (4, 1_700_000_000)] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(tls, &request)
                .unwrap();
            kernel
                .send_guest_sync_request(pid, tid, clocks[index])
                .unwrap();
            let mut output = [0u8; 0x100];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(tls, &mut output)
                .unwrap();
            let message = crate::hipc::parse(&output).unwrap();
            let response = crate::cmif::parse_response(message.raw_data).unwrap();
            assert_eq!(response.header.result, 0);
            assert_eq!(
                i64::from_le_bytes(response.payload[..8].try_into().unwrap()),
                expected
            );
            if index == 2 {
                assert_eq!(
                    &response.payload[8..24],
                    &kernel.time.steady_clock().source_id
                );
            }
        }
    }

    #[test]
    fn apm_open_session_returns_a_typed_guest_move_handle() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "apm").unwrap();
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&8u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(message.handles.move_handles.len(), 1);
        let returned = crate::handle::Handle(message.handles.move_handles[0]);
        let KernelObject::Session(id) = kernel.process(pid).unwrap().handles.get(returned).unwrap()
        else {
            panic!("APM must return a session");
        };
        assert!(matches!(
            kernel.sessions[id].target,
            SessionTarget::ApmSession
        ));
    }

    #[test]
    fn pointer_buffer_query_is_a_transport_command_not_a_service_method() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        for (service, capacity) in [
            ("appletOE", 0u16),
            ("hid", crate::hid_service::POINTER_BUFFER_SIZE),
        ] {
            let root = kernel.connect_service(pid, service).unwrap();
            for message_type in [3u32, 5, 7] {
                let mut request = [0u8; 0x100];
                request[..4].copy_from_slice(&message_type.to_le_bytes());
                request[4..8].copy_from_slice(&8u32.to_le_bytes());
                request[16..20].copy_from_slice(b"SFCI");
                request[24..28].copy_from_slice(&3u32.to_le_bytes());
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                kernel.send_guest_sync_request(pid, tid, root).unwrap();
                let mut output = [0u8; 0x100];
                kernel
                    .process(pid)
                    .unwrap()
                    .memory
                    .read(tls, &mut output)
                    .unwrap();
                let message = crate::hipc::parse(&output).unwrap();
                let response = crate::cmif::parse_response(message.raw_data).unwrap();
                assert_eq!(response.header.result, 0);
                assert_eq!(&response.payload[..2], &capacity.to_le_bytes());
                assert!(message.handles.move_handles.is_empty());
                // Unknown manager methods must not become false successes.
                request[24..28].copy_from_slice(&99u32.to_le_bytes());
                kernel
                    .process_mut(pid)
                    .unwrap()
                    .memory
                    .write(tls, &request)
                    .unwrap();
                assert!(kernel.send_guest_sync_request(pid, tid, root).is_err());
            }
        }
    }

    #[test]
    fn domain_conversion_routes_children_and_release_invalidates_only_its_object() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let root = kernel.connect_service(pid, "apm").unwrap();
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        let mut conversion = [0u8; 0x100];
        conversion[..4].copy_from_slice(&5u32.to_le_bytes());
        conversion[4..8].copy_from_slice(&8u32.to_le_bytes());
        conversion[16..20].copy_from_slice(b"SFCI");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &conversion)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(
            &crate::cmif::parse_response(message.raw_data)
                .unwrap()
                .payload[..4],
            &1u32.to_le_bytes()
        );
        let mut request = [0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&12u32.to_le_bytes());
        request[16] = 1;
        request[18..20].copy_from_slice(&16u16.to_le_bytes());
        request[20..24].copy_from_slice(&1u32.to_le_bytes());
        request[32..36].copy_from_slice(b"SFCI");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert!(message.handles.move_handles.is_empty());
        assert_eq!(&output[16..20], &1u32.to_le_bytes());
        assert_eq!(&output[32..36], b"SFCO");
        assert_eq!(&output[48..52], &2u32.to_le_bytes());
        let mut release = [0u8; 0x100];
        release[..4].copy_from_slice(&4u32.to_le_bytes());
        release[4..8].copy_from_slice(&8u32.to_le_bytes());
        release[16] = 2;
        release[20..24].copy_from_slice(&2u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &release)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        assert!(kernel.send_sync_request(pid, tid, root).is_err());
        // The root survives child release and allocates a fresh, non-reused ID.
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, root).unwrap();
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        assert_eq!(&output[48..52], &3u32.to_le_bytes());
    }

    #[test]
    fn open_logger_materializes_a_typed_move_handle() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("22.1.0".into()));
        let pid = kernel.create_process(1, "test");
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        let handle = kernel.connect_service(pid, "lm").unwrap();
        let mut request = vec![0u8; 0x100];
        request[..4].copy_from_slice(&4u32.to_le_bytes());
        request[4..8].copy_from_slice(&12u32.to_le_bytes());
        request[16..20].copy_from_slice(b"SFCI");
        request[32..40].copy_from_slice(&pid.to_le_bytes());
        let tls = kernel.thread(tid).unwrap().context.tpidrro_el0;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &request)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, handle).unwrap();
        let mut output = [0u8; 0x100];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(tls, &mut output)
            .unwrap();
        let message = crate::hipc::parse(&output).unwrap();
        assert_eq!(message.handles.move_handles.len(), 1);
        assert_eq!(&output[16..20], b"SFCO");
        let returned = crate::handle::Handle(message.handles.move_handles[0]);
        let session_id = match kernel.process(pid).unwrap().handles.get(returned).unwrap() {
            KernelObject::Session(id) => *id,
            _ => panic!("logger must be a session"),
        };
        assert!(
            matches!(&kernel.sessions[&session_id].target, SessionTarget::Logger(logger) if logger.process_id == pid)
        );
        let mut release = [0u8; 0x100];
        release[..4].copy_from_slice(&2u32.to_le_bytes());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &release)
            .unwrap();
        kernel.send_guest_sync_request(pid, tid, handle).unwrap();
        // The logger returned by OpenLogger remains independent of the root session.
        assert!(matches!(
            &kernel.sessions[&session_id].target,
            SessionTarget::Logger(_)
        ));
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(tls, &release)
            .unwrap();
        assert!(kernel.send_guest_sync_request(pid, tid, handle).is_err());
        kernel
            .process_mut(pid)
            .unwrap()
            .handles
            .close(handle)
            .unwrap();
    }

    #[test]
    fn empty_condition_signal_clears_flag_and_rejects_unsafe_waiter_path() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x8000,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::Data,
            )
            .unwrap();
        let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
        for count in [-1, 0, 1, 5] {
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(0x8000, &1u32.to_le_bytes())
                .unwrap();
            kernel.signal_process_wide_key(pid, 0x8000, count).unwrap();
            let mut bytes = [0xff; 4];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(0x8000, &mut bytes)
                .unwrap();
            assert_eq!(bytes, [0; 4]);
        }
        assert!(kernel.signal_process_wide_key(pid, 0x8001, -1).is_err());
        kernel.thread_mut(tid).unwrap().state = ThreadState::Waiting;
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .write(0x8000, &1u32.to_le_bytes())
            .unwrap();
        assert!(kernel.signal_process_wide_key(pid, 0x8000, -1).is_err());
        let mut bytes = [0; 4];
        kernel
            .process(pid)
            .unwrap()
            .memory
            .read(0x8000, &mut bytes)
            .unwrap();
        assert_eq!(bytes, 1u32.to_le_bytes());
        assert_eq!(kernel.thread(tid).unwrap().state, ThreadState::Waiting);
    }

    #[test]
    fn tls_slots_are_zeroed_distinct_and_cross_pages() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        let mut addresses = Vec::new();
        for index in 0..9 {
            let (tid, _) = kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap();
            let address = kernel.thread(tid).unwrap().context.tpidrro_el0;
            assert_eq!(address, 0x7000000000 + index * 0x200);
            let mut bytes = [0xff; 0x200];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(address, &mut bytes)
                .unwrap();
            assert_eq!(bytes, [0; 0x200]);
            kernel
                .process_mut(pid)
                .unwrap()
                .memory
                .write(address, &[index as u8 + 1])
                .unwrap();
            addresses.push(address);
        }
        for (index, address) in addresses.into_iter().enumerate() {
            let mut byte = [0];
            kernel
                .process(pid)
                .unwrap()
                .memory
                .read(address, &mut byte)
                .unwrap();
            assert_eq!(byte[0], index as u8 + 1);
        }
    }

    #[test]
    fn tls_mapping_failure_does_not_publish_a_partial_thread() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(1, "test");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x7000000000,
                PAGE_SIZE,
                MemoryPermission::READ,
                MemoryKind::Data,
            )
            .unwrap();
        assert!(kernel.create_thread(pid, 0x1000, 0x9000, 44).is_err());
        assert!(kernel.process(pid).unwrap().thread_ids.is_empty());
        assert!(kernel.thread(1).is_err());
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .unmap(0x7000000000, PAGE_SIZE)
            .unwrap();
        assert_eq!(kernel.create_thread(pid, 0x1000, 0x9000, 44).unwrap().0, 1);
    }

    #[test]
    fn process_thread_and_svc_smoke() {
        let mut kernel = HorizonKernel::new(HorizonProfileId("test".into()));
        let pid = kernel.create_process(0x100, "homebrew");
        kernel
            .process_mut(pid)
            .unwrap()
            .memory
            .map(
                0x1000,
                PAGE_SIZE,
                MemoryPermission::READ_EXECUTE,
                MemoryKind::Code,
            )
            .unwrap();
        let (tid, handle) = kernel.create_thread(pid, 0x1000, 0x8000, 0x2c).unwrap();
        kernel.start_thread(tid).unwrap();
        assert_eq!(kernel.ready_threads().count(), 1);
        let out = dispatch(&mut kernel, pid, tid, 0x25, SvcArguments::default()).unwrap();
        assert_eq!(out.x[1], tid);
        let mut close = SvcArguments::default();
        close.x[0] = u64::from(handle.0);
        dispatch(&mut kernel, pid, tid, 0x16, close).unwrap();
    }
}
