// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Stateful Maxwell trace → canonical GPU module lowering.
//!
//! The translator is intentionally conservative. GPU virtual addresses are resolved through the
//! address-space model; they are never used directly as `ResourceId`s.

use crate::{
    Command, FrontEndTrace, GpuModule, address_space::GpuAddressSpace, dma::DmaEvent,
    frontend::FrontEndEvent, maxwell_state::MaxwellEvent,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranslationStats {
    pub canonical_commands: usize,
    pub opaque_commands: usize,
    pub dma_launches: usize,
    pub unresolved_addresses: usize,
    pub macro_failures: usize,
    pub invalid_traces: usize,
}

/// Lowers commands whose semantics do not require GPU virtual-address resolution.
///
/// DMA launches are counted but intentionally left unresolved. Use [`lower_trace_with_address_space`]
/// for executable resource transfers.
pub fn lower_trace(trace: &FrontEndTrace) -> (GpuModule, TranslationStats) {
    lower_impl(trace, None)
}

/// Lowers a trace while resolving guest GPU virtual addresses into stable resources.
pub fn lower_trace_with_address_space(
    trace: &FrontEndTrace,
    address_space: &GpuAddressSpace,
) -> (GpuModule, TranslationStats) {
    lower_impl(trace, Some(address_space))
}

fn lower_impl(
    trace: &FrontEndTrace,
    address_space: Option<&GpuAddressSpace>,
) -> (GpuModule, TranslationStats) {
    let mut module = GpuModule::default();
    let mut stats = TranslationStats::default();
    if !trace.has_complete_event_order() {
        stats.invalid_traces = 1;
        stats.opaque_commands = 1;
        return (module, stats);
    }

    // Grouping by engine would move uploads after draws that consume them.
    for event in &trace.events {
        match *event {
            FrontEndEvent::Graphics(index) => match &trace.graphics_events[index] {
                MaxwellEvent::DrawVertexArray { first, count, .. } => {
                    module.commands.push(Command::Draw {
                        vertices: *count,
                        instances: 1,
                        first_vertex: *first,
                    });
                    stats.canonical_commands += 1;
                }
                MaxwellEvent::Barrier | MaxwellEvent::WaitForIdle => {
                    module.commands.push(Command::Barrier {
                        scope: crate::SyncScope::Queue,
                    });
                    stats.canonical_commands += 1;
                }
                MaxwellEvent::OpaqueRegister { method, argument } => {
                    module.commands.push(Command::Opaque {
                        class_id: 0xB197,
                        method: u32::from(*method),
                        argument: *argument,
                    });
                    stats.opaque_commands += 1;
                }
                MaxwellEvent::MacroFailed { .. } => {
                    stats.macro_failures += 1;
                    stats.opaque_commands += 1;
                    // Commands after this failure cannot be regarded as a
                    // successfully translated stream. Retain only its prefix.
                    break;
                }
                MaxwellEvent::DrawIndexBuffer { .. }
                | MaxwellEvent::SyncPointIncrement { .. }
                | MaxwellEvent::ConstantBufferUpload { .. } => {
                    // Neither indexed resource binding nor guest syncpoint
                    // completion is implemented here. Do not silently discard
                    // the operation and translate its dependent suffix.
                    stats.opaque_commands += 1;
                    break;
                }
                _ => {}
            },
            FrontEndEvent::Dma(index) => match &trace.dma_events[index] {
                DmaEvent::Launch(launch) => {
                    stats.dma_launches += 1;
                    let Some(shape) = launch.linear_shape() else {
                        stats.opaque_commands += 1;
                        continue;
                    };
                    // Bound expansion of untrusted multi-line descriptors. Reject
                    // the whole launch rather than emitting a truncated transfer.
                    if shape.rows > 65_536 {
                        stats.opaque_commands += 1;
                        continue;
                    }
                    let bytes = u64::from(shape.bytes_per_row);
                    let Some(address_space) = address_space else {
                        stats.unresolved_addresses += 2;
                        stats.opaque_commands += 1;
                        continue;
                    };
                    let source_address = launch.state.source.high_low();
                    let destination_address = launch.state.destination.high_low();
                    let mut copies = Vec::new();
                    let mut rejected = false;
                    for row in 0..shape.rows {
                        let source = source_address
                            .checked_add(u64::from(row) * u64::from(shape.source_stride))
                            .ok_or(crate::address_space::GpuAddressError::AddressOverflow)
                            .and_then(|address| address_space.resolve(address, bytes));
                        let destination = destination_address
                            .checked_add(u64::from(row) * u64::from(shape.destination_stride))
                            .ok_or(crate::address_space::GpuAddressError::AddressOverflow)
                            .and_then(|address| address_space.resolve(address, bytes));
                        match (source, destination) {
                            (Ok(source), Ok(destination))
                                if source.permissions.read && destination.permissions.write =>
                            {
                                copies.push(Command::Copy {
                                    source: source.resource,
                                    destination: destination.resource,
                                    source_offset: source.offset,
                                    destination_offset: destination.offset,
                                    bytes,
                                });
                            }
                            (source, destination) => {
                                stats.unresolved_addresses += usize::from(source.is_err())
                                    + usize::from(destination.is_err());
                                stats.opaque_commands += 1;
                                rejected = true;
                                break;
                            }
                        }
                    }
                    if !rejected {
                        stats.canonical_commands += copies.len();
                        module.commands.extend(copies);
                    }
                }
                DmaEvent::Opaque { .. } => stats.opaque_commands += 1,
            },
            // These engines are not lowered yet. Keep them visible in coverage
            // statistics rather than treating their events as implemented.
            FrontEndEvent::Compute(_)
            | FrontEndEvent::Inline(_)
            | FrontEndEvent::TwoD(_)
            | FrontEndEvent::Opaque(_) => stats.opaque_commands += 1,
            FrontEndEvent::HostSemaphore(_) => stats.opaque_commands += 1,
            FrontEndEvent::HostWaitForIdle { .. } => stats.opaque_commands += 1,
        }
    }

    (module, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        address_space::{GpuMapPermissions, GpuMapping},
        dma::{DmaCopyState, DmaLaunch},
        maxwell_state::RawGpuAddress,
        resource::ResourceId,
    };

    #[test]
    fn unsupported_indexed_draw_or_syncpoint_stops_translation_at_its_prefix() {
        for unsupported in [
            MaxwellEvent::DrawIndexBuffer {
                raw: 3,
                state: Box::default(),
            },
            MaxwellEvent::SyncPointIncrement { payload: 7 },
        ] {
            let trace = FrontEndTrace {
                graphics_events: vec![
                    MaxwellEvent::Barrier,
                    unsupported,
                    MaxwellEvent::DrawVertexArray {
                        first: 0,
                        count: 3,
                        state: Box::default(),
                    },
                ],
                events: vec![
                    FrontEndEvent::Graphics(0),
                    FrontEndEvent::Graphics(1),
                    FrontEndEvent::Graphics(2),
                ],
                ..FrontEndTrace::default()
            };
            let (module, stats) = lower_trace(&trace);
            assert!(matches!(
                module.commands.as_slice(),
                [Command::Barrier { .. }]
            ));
            assert_eq!(stats.canonical_commands, 1);
            assert_eq!(stats.opaque_commands, 1);
        }
    }

    #[test]
    fn dma_requires_address_space_instead_of_fabricating_resource_ids() {
        let mut trace = FrontEndTrace::default();
        trace.dma_events.push(DmaEvent::Launch(DmaLaunch {
            state: DmaCopyState {
                source: RawGpuAddress { a: 0, b: 0x1010 },
                destination: RawGpuAddress { a: 0, b: 0x2020 },
                line_length: 16,
                line_count: 1,
                ..DmaCopyState::default()
            },
            flags: 0x181,
        }));
        trace.events.push(FrontEndEvent::Dma(0));
        let (module, stats) = lower_trace(&trace);
        assert!(module.commands.is_empty());
        assert_eq!(stats.unresolved_addresses, 2);

        let mut space = GpuAddressSpace::new();
        space
            .map(GpuMapping {
                guest_base: 0x1000,
                size: 0x100,
                resource: ResourceId(11),
                resource_offset: 0x80,
                permissions: GpuMapPermissions::READ_ONLY,
            })
            .unwrap();
        space
            .map(GpuMapping {
                guest_base: 0x2000,
                size: 0x100,
                resource: ResourceId(12),
                resource_offset: 0x100,
                permissions: GpuMapPermissions::READ_WRITE,
            })
            .unwrap();
        let (module, stats) = lower_trace_with_address_space(&trace, &space);
        assert_eq!(stats.canonical_commands, 1);
        assert!(matches!(
            module.commands[0],
            Command::Copy {
                source: ResourceId(11),
                destination: ResourceId(12),
                source_offset: 0x90,
                destination_offset: 0x120,
                bytes: 16
            }
        ));

        trace.graphics_events.extend([
            MaxwellEvent::Barrier,
            MaxwellEvent::DrawVertexArray {
                first: 3,
                count: 4,
                state: Box::default(),
            },
        ]);
        trace.events = vec![
            FrontEndEvent::Graphics(0),
            FrontEndEvent::Dma(0),
            FrontEndEvent::Graphics(1),
        ];
        let (module, stats) = lower_trace_with_address_space(&trace, &space);
        assert_eq!(stats.canonical_commands, 3);
        assert!(matches!(
            module.commands.as_slice(),
            [
                Command::Barrier { .. },
                Command::Copy { .. },
                Command::Draw {
                    first_vertex: 3,
                    vertices: 4,
                    ..
                },
            ]
        ));
        trace.events.push(FrontEndEvent::Inline(0));
        trace
            .inline_events
            .push(crate::inline_to_memory::InlineToMemoryEvent::Data { value: 1 });
        let (_, stats) = lower_trace_with_address_space(&trace, &space);
        assert_eq!(stats.opaque_commands, 1);

        let DmaEvent::Launch(launch) = &mut trace.dma_events[0] else {
            unreachable!()
        };
        launch.flags = 0x381;
        launch.state.line_count = 2;
        launch.state.pitch_in = 32;
        launch.state.pitch_out = 64;
        trace.events = vec![FrontEndEvent::Dma(0)];
        trace.graphics_events.clear();
        trace.inline_events.clear();
        let (module, stats) = lower_trace_with_address_space(&trace, &space);
        assert_eq!(stats.canonical_commands, 2);
        assert!(matches!(
            module.commands[1],
            Command::Copy {
                source_offset: 0xb0,
                destination_offset: 0x160,
                ..
            }
        ));
        let DmaEvent::Launch(launch) = &mut trace.dma_events[0] else {
            unreachable!()
        };
        launch.state.pitch_out = 0x1000;
        let (module, stats) = lower_trace_with_address_space(&trace, &space);
        assert!(module.commands.is_empty());
        assert_eq!(stats.opaque_commands, 1);
        assert_eq!(stats.unresolved_addresses, 1);
    }

    #[test]
    fn malformed_event_order_is_reported_before_lowering_any_commands() {
        let mut trace = FrontEndTrace {
            graphics_events: vec![
                MaxwellEvent::Barrier,
                MaxwellEvent::DrawVertexArray {
                    first: 0,
                    count: 3,
                    state: Box::default(),
                },
            ],
            ..FrontEndTrace::default()
        };
        for events in [
            vec![],
            vec![FrontEndEvent::Graphics(0)],
            vec![FrontEndEvent::Graphics(0), FrontEndEvent::Graphics(0)],
            vec![FrontEndEvent::Graphics(1), FrontEndEvent::Graphics(0)],
            vec![FrontEndEvent::Graphics(0), FrontEndEvent::Graphics(9)],
        ] {
            trace.events = events;
            let (module, stats) = lower_trace(&trace);
            assert!(module.commands.is_empty());
            assert_eq!(stats.invalid_traces, 1);
            assert_eq!(stats.canonical_commands, 0);
        }
        let (_, stats) = lower_trace(&FrontEndTrace::default());
        assert_eq!(stats.invalid_traces, 0);
    }
}
