// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Kernel event primitive.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Manual-clear state shared by a service, copied handles and pending waits.
/// Equality denotes object identity, never the current signal value.
#[derive(Debug, Clone, Default)]
pub struct SharedEvent(Arc<AtomicBool>);

impl PartialEq for SharedEvent {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for SharedEvent {}

impl SharedEvent {
    pub fn is_signaled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn set_signaled(&self, signaled: bool) {
        self.0.store(signaled, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct KernelEvent {
    pub id: u64,
    pub signaled: bool,
    pub auto_clear: bool,
}
impl KernelEvent {
    pub fn new(id: u64, auto_clear: bool) -> Self {
        Self {
            id,
            signaled: false,
            auto_clear,
        }
    }
    pub fn signal(&mut self) {
        self.signaled = true
    }
    pub fn poll(&mut self) -> bool {
        let v = self.signaled;
        if v && self.auto_clear {
            self.signaled = false;
        }
        v
    }
    pub fn clear(&mut self) {
        self.signaled = false
    }
}
