// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Horizon user process model.

use crate::{handle::HandleTable, memory::AddressSpace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Running,
    Exited,
}

#[derive(Debug, Clone)]
pub struct HorizonProcess {
    pub id: u64,
    pub program_id: u64,
    pub name: String,
    pub state: ProcessState,
    pub memory: AddressSpace,
    pub layout: Option<crate::layout::ProcessLayout>,
    pub handles: HandleTable,
    pub thread_ids: Vec<u64>,
    pub application: crate::applet::ApplicationState,
    /// Explicit guest scheduling grants; absent for unconfigured synthetic processes.
    pub scheduling: Option<ProcessSchedulingCapabilities>,
    /// Guest NPDM default core, independent of the host scheduler.
    pub ideal_core: Option<u8>,
    // Custom Debug redacts seeds from kernel diagnostic dumps.
    entropy: ProcessEntropy,
    next_tls_slot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessSchedulingCapabilities {
    pub core_mask: u64,
    pub priority_mask: u64,
}

#[derive(Clone, Default)]
struct ProcessEntropy(Option<[u64; 4]>);

impl std::fmt::Debug for ProcessEntropy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProcessEntropy(<redacted>)")
    }
}

impl HorizonProcess {
    /// Charged byte-backed code/data, stack and heap mappings in the current
    /// diagnostic loader. Virtual reservations and TLS are not heap allocations.
    /// Shared/alias backing will require unique-page accounting when introduced.
    pub fn used_memory_size(&self) -> crate::error::HorizonResult<u64> {
        self.memory.used_memory_size()
    }
    pub fn new(id: u64, program_id: u64, name: impl Into<String>) -> Self {
        Self {
            id,
            program_id,
            name: name.into(),
            state: ProcessState::Created,
            memory: AddressSpace::default(),
            layout: None,
            handles: HandleTable::default(),
            thread_ids: Vec::new(),
            // Diagnostic resource identities are unique for the kernel lifetime;
            // do not use a title ID, which may be shared by multiple processes.
            application: crate::applet::ApplicationState::new(id),
            scheduling: None,
            ideal_core: None,
            entropy: ProcessEntropy::default(),
            next_tls_slot: 0,
        }
    }

    /// Horizon exposes four stable seed words for each process, not a new draw per call.
    pub fn random_entropy(&mut self, index: u64) -> crate::error::HorizonResult<u64> {
        let index = usize::try_from(index)
            .ok()
            .filter(|index| *index < 4)
            .ok_or_else(|| {
                crate::error::HorizonError::InvalidState("entropy index must be 0..3".into())
            })?;
        if self.entropy.0.is_none() {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|error| {
                crate::error::HorizonError::InvalidState(format!(
                    "host entropy unavailable: {error}"
                ))
            })?;
            self.entropy.0 = Some(std::array::from_fn(|index| {
                u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap())
            }));
        }
        Ok(self.entropy.0.as_ref().unwrap()[index])
    }

    /// Allocate one zeroed 0x200-byte thread-local region, eight per backing page.
    /// Slots are not reused yet: exiting a thread must not expose stale bytes to another.
    pub(crate) fn allocate_tls(&mut self) -> crate::error::HorizonResult<u64> {
        use crate::{
            error::HorizonError,
            memory::{MemoryKind, MemoryPermission, PAGE_SIZE},
        };
        const TLS_SIZE: u64 = 0x200;
        let region = self
            .layout
            .map(|layout| layout.tls)
            .unwrap_or(crate::layout::VirtualRegion {
                base: 0x70_0000_0000,
                size: 0x10_0000,
            });
        if self.next_tls_slot >= region.size / TLS_SIZE {
            return Err(HorizonError::InvalidState(
                "thread-local region exhausted".into(),
            ));
        }
        let address = region.base + self.next_tls_slot * TLS_SIZE;
        if address % PAGE_SIZE == 0 {
            self.memory.map(
                address,
                PAGE_SIZE,
                MemoryPermission::READ_WRITE,
                MemoryKind::ThreadLocal,
            )?;
        }
        self.next_tls_slot += 1;
        Ok(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_memory_tracks_charged_mappings_not_tls_or_reservations() {
        use crate::memory::{MemoryKind, MemoryPermission, PAGE_SIZE};
        let mut process = HorizonProcess::new(1, 1, "test");
        for (index, kind) in [
            MemoryKind::Code,
            MemoryKind::Data,
            MemoryKind::Stack,
            MemoryKind::Heap,
            MemoryKind::ThreadLocal,
        ]
        .into_iter()
        .enumerate()
        {
            process
                .memory
                .map(
                    (index as u64 + 1) * PAGE_SIZE,
                    PAGE_SIZE,
                    MemoryPermission::READ_WRITE,
                    kind,
                )
                .unwrap();
        }
        assert_eq!(process.used_memory_size().unwrap(), 4 * PAGE_SIZE);
        process.memory.unmap(4 * PAGE_SIZE, PAGE_SIZE).unwrap();
        assert_eq!(process.used_memory_size().unwrap(), 3 * PAGE_SIZE);
    }

    #[test]
    fn entropy_is_stable_and_invalid_indices_do_not_initialize_it() {
        let mut process = HorizonProcess::new(1, 1, "test");
        assert!(process.random_entropy(4).is_err());
        assert!(process.random_entropy(u64::MAX).is_err());
        assert!(process.entropy.0.is_none());
        for index in 0..4 {
            let first = process.random_entropy(index).unwrap();
            assert_eq!(first, process.random_entropy(index).unwrap());
        }
        assert_eq!(
            format!("{:?}", process.entropy),
            "ProcessEntropy(<redacted>)"
        );
    }
}
