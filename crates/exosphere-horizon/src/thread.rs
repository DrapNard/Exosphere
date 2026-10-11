// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Horizon thread state and guest CPU context.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Created,
    Ready,
    Running,
    Waiting,
    Exited,
}

/// Minimal AArch64 user context shared with the CPU runtime boundary.
#[derive(Debug, Clone, Default)]
pub struct ThreadContext {
    pub exclusive: Option<ExclusiveReservation>,
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    /// SIMD/FP registers: lane zero occupies the least-significant bits.
    pub v: [u128; 32],
    pub fpcr: u32,
    pub fpsr: u32,
    pub tpidr_el0: u64,
    /// Kernel-owned address of the Horizon 0x200-byte thread-local region.
    pub tpidrro_el0: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExclusiveReservation {
    pub address: u64,
    pub size: usize,
    pub epoch: u64,
}

#[derive(Debug, Clone)]
pub struct HorizonThread {
    pub id: u64,
    pub process_id: u64,
    pub name: String,
    pub priority: i32,
    /// Requested priority before mutex-owner inheritance.
    pub base_priority: i32,
    pub affinity: Option<ThreadAffinity>,
    pub state: ThreadState,
    pub context: ThreadContext,
    /// Thread-local IPC command buffer used by SendSyncRequest.
    pub ipc_buffer: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadAffinity {
    /// -1 means no preferred guest core; otherwise an original-Switch core 0..3.
    pub ideal_core: i32,
    pub mask: u64,
}

impl HorizonThread {
    pub fn new(id: u64, process_id: u64, entry: u64, stack_top: u64, priority: i32) -> Self {
        let context = ThreadContext {
            pc: entry,
            sp: stack_top,
            ..Default::default()
        };
        Self {
            id,
            process_id,
            name: format!("Thread-{id}"),
            priority,
            base_priority: priority,
            affinity: None,
            state: ThreadState::Created,
            context,
            ipc_buffer: vec![0; 0x100],
        }
    }
}
