// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! End-to-end Maxwell command-stream front-end.
//!
//! PFIFO parsing is followed by subchannel routing and stateful engine tracking. The resulting trace
//! is intentionally lossless: known semantics are promoted to typed events while unknown writes are
//! preserved for diagnostics and future coverage work.

use crate::{
    PfifoError,
    compute::{ComputeEvent, ComputeState},
    decode_pushbuffer,
    dma::{DmaCopyState, DmaEvent},
    engine::{EngineClass, EngineMethodWrite, SubchannelTracker},
    inline_to_memory::{InlineToMemoryEvent, InlineToMemoryState},
    maxwell_state::{Maxwell3dState, MaxwellEvent},
    twod::{TwoDEvent, TwoDState},
};

#[derive(Debug, Clone)]
pub struct OpaqueEngineWrite {
    pub write: EngineMethodWrite,
}

/// An event's position in its engine-specific vector. Consumers must traverse this
/// sequence, not concatenate the vectors: transfers and draws can be interleaved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontEndEvent {
    Graphics(usize),
    Compute(usize),
    Dma(usize),
    Inline(usize),
    TwoD(usize),
    Opaque(usize),
    HostSemaphore(usize),
    HostWaitForIdle { scope: u32 },
}

#[derive(Debug, Clone, Default)]
pub struct FrontEndTrace {
    pub method_writes: usize,
    pub routed_writes: usize,
    /// Front-end emission order, including macro events at the existing invocation
    /// boundaries. This does not establish asynchronous hardware completion order.
    pub events: Vec<FrontEndEvent>,
    pub graphics_events: Vec<MaxwellEvent>,
    pub compute_events: Vec<ComputeEvent>,
    pub dma_events: Vec<DmaEvent>,
    pub inline_events: Vec<InlineToMemoryEvent>,
    pub twod_events: Vec<TwoDEvent>,
    pub opaque_engine_writes: Vec<OpaqueEngineWrite>,
    pub host_semaphore_operations: Vec<crate::host_semaphore::HostSemaphoreOperation>,
    pub graphics_state: Maxwell3dState,
    pub compute_state: ComputeState,
    pub dma_state: DmaCopyState,
    pub inline_state: InlineToMemoryState,
    pub twod_state: TwoDState,
}

impl FrontEndTrace {
    /// Validate public trace construction before an executor trusts event indices.
    /// Each per-engine event must appear exactly once and in engine-local order.
    pub fn has_complete_event_order(&self) -> bool {
        let lengths = [
            self.graphics_events.len(),
            self.compute_events.len(),
            self.dma_events.len(),
            self.inline_events.len(),
            self.twod_events.len(),
            self.opaque_engine_writes.len(),
            self.host_semaphore_operations.len(),
        ];
        let mut next = [0; 7];
        for event in &self.events {
            let (engine, index) = match *event {
                FrontEndEvent::Graphics(index) => (0, index),
                FrontEndEvent::Compute(index) => (1, index),
                FrontEndEvent::Dma(index) => (2, index),
                FrontEndEvent::Inline(index) => (3, index),
                FrontEndEvent::TwoD(index) => (4, index),
                FrontEndEvent::Opaque(index) => (5, index),
                FrontEndEvent::HostSemaphore(index) => (6, index),
                FrontEndEvent::HostWaitForIdle { .. } => continue,
            };
            if index != next[engine] || index >= lengths[engine] {
                return false;
            }
            next[engine] += 1;
        }
        next == lengths
    }

    fn push_graphics(&mut self, event: MaxwellEvent) {
        self.events
            .push(FrontEndEvent::Graphics(self.graphics_events.len()));
        self.graphics_events.push(event);
    }
}

#[derive(Debug, Default)]
pub struct MaxwellFrontEnd {
    subchannels: SubchannelTracker,
    graphics: Maxwell3dState,
    compute: ComputeState,
    dma: DmaCopyState,
    inline: InlineToMemoryState,
    twod: TwoDState,
    host_semaphore: crate::host_semaphore::HostSemaphoreState,
}

impl MaxwellFrontEnd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_pushbuffer(&mut self, words: &[u32]) -> Result<FrontEndTrace, PfifoError> {
        let writes = decode_pushbuffer(words)?;
        let mut trace = FrontEndTrace {
            method_writes: writes.len(),
            ..FrontEndTrace::default()
        };

        for write in writes {
            // NVB06F_NOP belongs to the FIFO, not the bound engine. Its handle
            // payload is ignored; routing it would invent an opaque GPU effect.
            if write.method == 2 {
                continue;
            }
            // NVIDIA NVB06F_WFI is byte method 0x78 (word method 0x1e).
            // It belongs to the FIFO, independently of the bound engine.
            // https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/master/classes/host/clb06f.h
            if write.method == 0x1e {
                if self.graphics.active_macro.is_some() {
                    for event in self.graphics.execute_active_macro(4096, 1_000_000, 65_536) {
                        trace.push_graphics(event);
                    }
                }
                trace.events.push(FrontEndEvent::HostWaitForIdle {
                    scope: write.argument,
                });
                continue;
            }
            if crate::host_semaphore::HostSemaphoreState::handles(write.method) {
                // Common FIFO methods are not owned by the currently bound
                // engine class. Finish prior macro emissions before a release.
                if self.graphics.active_macro.is_some() {
                    for event in self.graphics.execute_active_macro(4096, 1_000_000, 65_536) {
                        trace.push_graphics(event);
                    }
                }
                if let Some(operation) = self.host_semaphore.apply(write.method, write.argument) {
                    trace.events.push(FrontEndEvent::HostSemaphore(
                        trace.host_semaphore_operations.len(),
                    ));
                    trace.host_semaphore_operations.push(operation);
                }
                continue;
            }
            // A different engine or a SetObject rebinding also ends the current
            // graphics invocation. Flush before routing: otherwise its effects
            // (or failure) would appear after the following engine's commands.
            if self.graphics.active_macro.is_some()
                && (write.method == 0
                    || self.subchannels.class_for(write.subchannel) != Some(EngineClass::Graphics))
            {
                for event in self.graphics.execute_active_macro(4096, 1_000_000, 65_536) {
                    trace.push_graphics(event);
                }
            }
            let Some(routed) = self.subchannels.route(write) else {
                continue;
            };
            trace.routed_writes += 1;
            match routed.class {
                EngineClass::Graphics => {
                    // A macro invocation has no explicit end marker.  The next non-macro PGRAPH
                    // method is therefore a safe ordering boundary: replay all accumulated macro
                    // emissions before applying that following method.  Starting another macro is
                    // also a boundary for the previous invocation.
                    let starts_new_macro = Maxwell3dState::is_macro_method(routed.method)
                        && routed
                            .method
                            .saturating_sub(crate::maxwell_state::method::MACRO_BASE)
                            % 2
                            == 0;
                    if self.graphics.active_macro.is_some()
                        && (!Maxwell3dState::is_macro_method(routed.method) || starts_new_macro)
                    {
                        for event in self.graphics.execute_active_macro(4096, 1_000_000, 65_536) {
                            trace.push_graphics(event);
                        }
                    }
                    if let Some(event) = self.graphics.apply(routed) {
                        trace.push_graphics(event);
                    }
                }
                EngineClass::Compute => {
                    if let Some(event) = self.compute.apply(routed) {
                        trace
                            .events
                            .push(FrontEndEvent::Compute(trace.compute_events.len()));
                        trace.compute_events.push(event);
                    }
                }
                EngineClass::DmaCopy => {
                    if let Some(event) = self.dma.apply(routed) {
                        trace
                            .events
                            .push(FrontEndEvent::Dma(trace.dma_events.len()));
                        trace.dma_events.push(event);
                    }
                }
                EngineClass::InlineToMemory => {
                    if let Some(event) = self.inline.apply(routed) {
                        trace
                            .events
                            .push(FrontEndEvent::Inline(trace.inline_events.len()));
                        trace.inline_events.push(event);
                    }
                }
                EngineClass::TwoD => {
                    if let Some(event) = self.twod.apply(routed) {
                        trace
                            .events
                            .push(FrontEndEvent::TwoD(trace.twod_events.len()));
                        trace.twod_events.push(event);
                    }
                }
                EngineClass::Unknown(_) => {
                    trace
                        .events
                        .push(FrontEndEvent::Opaque(trace.opaque_engine_writes.len()));
                    trace
                        .opaque_engine_writes
                        .push(OpaqueEngineWrite { write: routed });
                }
            }
        }

        // Pushbuffer completion is the final invocation boundary for a trailing macro.
        if self.graphics.active_macro.is_some() {
            for event in self.graphics.execute_active_macro(4096, 1_000_000, 65_536) {
                trace.push_graphics(event);
            }
        }

        trace.graphics_state = self.graphics.clone();
        trace.compute_state = self.compute;
        trace.dma_state = self.dma;
        trace.inline_state = self.inline.clone();
        trace.twod_state = self.twod.clone();
        Ok(trace)
    }

    pub fn graphics_state(&self) -> &Maxwell3dState {
        &self.graphics
    }
    pub fn compute_state(&self) -> &ComputeState {
        &self.compute
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{CLASS_MAXWELL_B, CLASS_MAXWELL_COMPUTE_B};
    fn immediate(method: u16, subchannel: u8, data: u32) -> u32 {
        u32::from(method) | (u32::from(subchannel) << 13) | ((data & 0x1fff) << 16) | (4 << 29)
    }
    fn non_incrementing(method: u16, subchannel: u8, count: u32) -> u32 {
        u32::from(method) | (u32::from(subchannel) << 13) | (count << 16) | (3 << 29)
    }
    #[test]
    fn routes_multiple_engines() {
        let words = vec![
            non_incrementing(0, 0, 1),
            u32::from(CLASS_MAXWELL_B),
            immediate(crate::maxwell_state::method::BEGIN, 0, 4),
            non_incrementing(0, 1, 1),
            u32::from(CLASS_MAXWELL_COMPUTE_B),
            immediate(crate::compute::method::SET_QMD_VERSION, 1, 2),
        ];
        let trace = MaxwellFrontEnd::new().process_pushbuffer(&words).unwrap();
        assert_eq!(trace.graphics_state.primitive_raw, Some(4));
        assert_eq!(trace.compute_state.qmd_version, 2);
    }

    #[test]
    fn fifo_nop_is_independent_of_subchannel_binding_and_payload() {
        let mut frontend = MaxwellFrontEnd::new();
        for binding in [None, Some(CLASS_MAXWELL_B), Some(0xffff)] {
            for subchannel in 0..8 {
                if let Some(class) = binding {
                    frontend
                        .process_pushbuffer(&[non_incrementing(0, subchannel, 1), u32::from(class)])
                        .unwrap();
                }
                let trace = frontend
                    .process_pushbuffer(&[
                        non_incrementing(2, subchannel, 3),
                        0,
                        u32::MAX,
                        0x12345678,
                    ])
                    .unwrap();
                assert_eq!(trace.method_writes, 3);
                assert_eq!(trace.routed_writes, 0);
                assert!(trace.events.is_empty());
                assert!(trace.has_complete_event_order());
            }
        }
    }

    #[test]
    fn pending_macro_failure_precedes_other_engine_and_rebinding() {
        for rebind in [false, true] {
            let mut words = vec![
                non_incrementing(0, 0, 1),
                u32::from(CLASS_MAXWELL_B),
                non_incrementing(0, 1, 1),
                0xffff,
                immediate(crate::maxwell_state::method::MACRO_BASE, 0, 7),
            ];
            let target = if rebind {
                words.extend([non_incrementing(0, 0, 1), 0xffff]);
                0
            } else {
                1
            };
            words.push(immediate(0xfff, target, 3));
            let trace = MaxwellFrontEnd::new().process_pushbuffer(&words).unwrap();
            assert!(trace.has_complete_event_order());
            assert!(matches!(
                trace.graphics_events[1],
                MaxwellEvent::MacroFailed { .. }
            ));
            assert_eq!(
                trace.events,
                vec![
                    FrontEndEvent::Graphics(0),
                    FrontEndEvent::Graphics(1),
                    FrontEndEvent::Opaque(0)
                ]
            );
        }
    }

    #[test]
    fn failed_macro_is_ordered_and_stops_lowering_before_following_draw() {
        let words = [
            non_incrementing(0, 0, 1),
            u32::from(CLASS_MAXWELL_B),
            immediate(crate::maxwell_state::method::MACRO_BASE, 0, 7),
            immediate(crate::maxwell_state::method::DRAW_VERTEX_ARRAY, 0, 3),
        ];
        let trace = MaxwellFrontEnd::new().process_pushbuffer(&words).unwrap();
        assert!(matches!(
            trace.graphics_events.as_slice(),
            [
                MaxwellEvent::MacroStarted { .. },
                MaxwellEvent::MacroFailed { .. },
                MaxwellEvent::DrawVertexArray { .. },
            ]
        ));
        assert_eq!(
            trace.events,
            vec![
                FrontEndEvent::Graphics(0),
                FrontEndEvent::Graphics(1),
                FrontEndEvent::Graphics(2)
            ]
        );
        let (module, stats) = crate::lower_trace(&trace);
        assert!(module.commands.is_empty());
        assert_eq!(stats.macro_failures, 1);
        assert_eq!(stats.opaque_commands, 1);
    }

    #[test]
    fn preserves_interleaved_engine_event_order() {
        use crate::engine::{
            CLASS_FERMI_TWOD_A, CLASS_KEPLER_INLINE_TO_MEMORY_B, CLASS_MAXWELL_DMA_COPY_A,
        };
        let classes = [
            CLASS_MAXWELL_B,
            CLASS_MAXWELL_COMPUTE_B,
            CLASS_MAXWELL_DMA_COPY_A,
            CLASS_KEPLER_INLINE_TO_MEMORY_B,
            CLASS_FERMI_TWOD_A,
            0xffff,
        ];
        let mut words = Vec::new();
        for (subchannel, class) in classes.into_iter().enumerate() {
            words.extend([non_incrementing(0, subchannel as u8, 1), u32::from(class)]);
        }
        // Revisit engines out of class-binding order. Opaque methods must remain
        // in the sequence as well: an executor cannot silently skip them.
        for subchannel in [3, 0, 2, 3, 1, 5, 4, 0] {
            words.push(immediate(0xfff, subchannel, u32::from(subchannel)));
        }
        let trace = MaxwellFrontEnd::new().process_pushbuffer(&words).unwrap();
        assert_eq!(
            trace.events,
            vec![
                FrontEndEvent::Inline(0),
                FrontEndEvent::Graphics(0),
                FrontEndEvent::Dma(0),
                FrontEndEvent::Inline(1),
                FrontEndEvent::Compute(0),
                FrontEndEvent::Opaque(0),
                FrontEndEvent::TwoD(0),
                FrontEndEvent::Graphics(1),
            ]
        );
        assert_eq!(trace.method_writes, 14);
        assert_eq!(trace.routed_writes, 8);
        assert_eq!(trace.graphics_events.len(), 2);
        assert_eq!(trace.inline_events.len(), 2);
    }
}
