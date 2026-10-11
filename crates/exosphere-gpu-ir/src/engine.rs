// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Maxwell subchannel/class tracker.
//!
//! PFIFO only yields method writes. This stage tracks `SetObject` so each following method can be
//! routed to the correct graphics/compute/2D/DMA engine without coupling PFIFO parsing to engine IR.

use crate::pfifo::MethodWrite;

pub const CLASS_MAXWELL_B: u16 = 0xB197;
pub const CLASS_MAXWELL_COMPUTE_B: u16 = 0xB1C0;
pub const CLASS_KEPLER_INLINE_TO_MEMORY_B: u16 = 0xA140;
pub const CLASS_FERMI_TWOD_A: u16 = 0x902D;
pub const CLASS_MAXWELL_DMA_COPY_A: u16 = 0xB0B5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineClass {
    Graphics,
    Compute,
    InlineToMemory,
    TwoD,
    DmaCopy,
    Unknown(u16),
}

impl EngineClass {
    pub fn from_class_id(class_id: u16) -> Self {
        match class_id {
            CLASS_MAXWELL_B => Self::Graphics,
            CLASS_MAXWELL_COMPUTE_B => Self::Compute,
            CLASS_KEPLER_INLINE_TO_MEMORY_B => Self::InlineToMemory,
            CLASS_FERMI_TWOD_A => Self::TwoD,
            CLASS_MAXWELL_DMA_COPY_A => Self::DmaCopy,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMethodWrite {
    pub subchannel: u8,
    pub class: EngineClass,
    pub method: u16,
    pub argument: u32,
}

#[derive(Debug, Default, Clone)]
pub struct SubchannelTracker {
    classes: [Option<u16>; 8],
}

impl SubchannelTracker {
    pub fn route(&mut self, write: MethodWrite) -> Option<EngineMethodWrite> {
        let slot = usize::from(write.subchannel);
        if write.method == 0 {
            self.classes[slot] = Some((write.argument & 0xffff) as u16);
            return None;
        }
        let class = self.classes[slot]
            .map(EngineClass::from_class_id)
            .unwrap_or(EngineClass::Unknown(0));
        Some(EngineMethodWrite {
            subchannel: write.subchannel,
            class,
            method: write.method,
            argument: write.argument,
        })
    }

    pub fn class_for(&self, subchannel: u8) -> Option<EngineClass> {
        self.classes[usize::from(subchannel)].map(EngineClass::from_class_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_object_binds_subchannel() {
        let mut tracker = SubchannelTracker::default();
        assert!(
            tracker
                .route(MethodWrite {
                    subchannel: 0,
                    method: 0,
                    argument: u32::from(CLASS_MAXWELL_B)
                })
                .is_none()
        );
        let routed = tracker
            .route(MethodWrite {
                subchannel: 0,
                method: 0x40,
                argument: 0,
            })
            .unwrap();
        assert_eq!(routed.class, EngineClass::Graphics);
    }
}
