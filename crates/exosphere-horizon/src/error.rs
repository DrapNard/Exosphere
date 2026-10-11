// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Errors produced by the Horizon compatibility kernel.

use thiserror::Error;

/// Kernel-level errors deliberately remain host-independent.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HorizonError {
    #[error("invalid address 0x{0:016x}")]
    InvalidAddress(u64),
    #[error("memory range overlaps an existing mapping")]
    MemoryOverlap,
    #[error("memory permission denied")]
    PermissionDenied,
    #[error("invalid or stale handle 0x{0:08x}")]
    InvalidHandle(u32),
    #[error("handle table is full")]
    HandleTableFull,
    #[error("unknown SVC 0x{0:02x}")]
    UnknownSvc(u16),
    #[error("unknown service '{0}'")]
    UnknownService(String),
    #[error("unknown IPC command {command_id} for service '{service}'")]
    UnknownCommand { service: String, command_id: u32 },
    #[error("filesystem error: {0}")]
    FileSystem(String),
    #[error("IPC protocol error: {0}")]
    IpcProtocol(String),
    #[error("invalid kernel state: {0}")]
    InvalidState(String),
}

pub type HorizonResult<T> = Result<T, HorizonError>;
