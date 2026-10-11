// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Conservative stateless Maxwell method decoder.
//!
//! Stateful register interpretation lives in `maxwell_state`. This module only lowers methods whose
//! meaning is independent of previous register writes. Unknown class/method pairs remain opaque.

use crate::Command;

pub const CLASS_3D: u32 = 0xB197;
pub const CLASS_COMPUTE: u32 = 0xB1C0;

#[derive(Debug, Clone, Copy)]
pub struct MaxwellMethod {
    pub class_id: u32,
    pub method: u32,
    pub argument: u32,
}

pub fn decode(method: MaxwellMethod) -> Command {
    // MAXWELL_B 0x585/0x586 are End/Begin, not draw calls. They are intentionally left to the
    // stateful front-end, where primitive state and macro-expanded draw methods can be combined.
    Command::Opaque {
        class_id: method.class_id,
        method: method.method,
        argument: method.argument,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_is_never_fabricated_into_draw() {
        assert!(matches!(
            decode(MaxwellMethod {
                class_id: CLASS_3D,
                method: 0x585,
                argument: 99
            }),
            Command::Opaque { .. }
        ));
    }
}
