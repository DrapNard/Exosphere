// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Canonical synchronization events produced by GPU engine front-ends.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuSyncEvent {
    WaitForIdle,
    IncrementSyncPoint { payload: u32 },
    SemaphoreSignal { address: u64, payload: u32 },
    Barrier,
}
