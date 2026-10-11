// SPDX-License-Identifier: GPL-3.0-or-later
// Project: Exosphere
// Author/Maintainer: DrapNard

//! Host-neutral HID state fed by Exosphere hypervisor/device plugins.
//!
//! The final Horizon shared-memory layout is a serialization concern. This model keeps host input
//! normalized first so Linux evdev, Windows Raw Input and macOS events share identical semantics.

use std::collections::BTreeSet;
pub mod ring;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseState {
    pub x: i64,
    pub y: i64,
    pub wheel_x: i64,
    pub wheel_y: i64,
    pub buttons: u32,
}

#[derive(Debug, Default, Clone)]
pub struct HidState {
    keyboard_usages: BTreeSet<u32>,
    pub mouse: MouseState,
    pub sampling_number: u64,
    pub primary_npad: NpadState,
}

/// Normalized controller sample. Publishing it to Horizon shared memory is a
/// separate operation; host input alone does not imply a guest-visible update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NpadState {
    pub buttons: u64,
    pub left_stick: [i32; 2],
    pub right_stick: [i32; 2],
    pub connected: bool,
}

impl NpadState {
    /// Serialize HidNpadCommonState, including its alignment padding. Ring-slot
    /// sequence numbers and ring headers must be published separately by HID.
    pub fn encode_common_sample(self, sampling_number: u64) -> [u8; 0x28] {
        let mut bytes = [0; 0x28];
        bytes[..8].copy_from_slice(&sampling_number.to_le_bytes());
        if self.connected {
            bytes[8..16].copy_from_slice(&self.buttons.to_le_bytes());
            for (index, axis) in self
                .left_stick
                .into_iter()
                .chain(self.right_stick)
                .enumerate()
            {
                let start = 16 + index * 4;
                bytes[start..start + 4].copy_from_slice(&axis.clamp(-32767, 32767).to_le_bytes());
            }
            // NpadAttribute_IsConnected; no wired/battery/physical-device claim.
            bytes[32..36].copy_from_slice(&1u32.to_le_bytes());
        }
        bytes
    }
}

impl HidState {
    pub fn set_primary_npad(&mut self, mut state: NpadState) {
        for axis in state
            .left_stick
            .iter_mut()
            .chain(state.right_stick.iter_mut())
        {
            *axis = (*axis).clamp(-32767, 32767);
        }
        if !state.connected {
            state.buttons = 0;
            state.left_stick = [0; 2];
            state.right_stick = [0; 2];
        }
        if self.primary_npad != state {
            self.primary_npad = state;
            self.bump_sample();
        }
    }
    pub fn key(&mut self, usage: u32, pressed: bool) {
        if pressed {
            self.keyboard_usages.insert(usage);
        } else {
            self.keyboard_usages.remove(&usage);
        }
        self.bump_sample();
    }

    pub fn mouse_motion(&mut self, dx: i32, dy: i32) {
        self.mouse.x = self.mouse.x.saturating_add(i64::from(dx));
        self.mouse.y = self.mouse.y.saturating_add(i64::from(dy));
        self.bump_sample();
    }

    pub fn mouse_button(&mut self, button: u32, pressed: bool) {
        if button < 32 {
            let bit = 1u32 << button;
            if pressed {
                self.mouse.buttons |= bit;
            } else {
                self.mouse.buttons &= !bit;
            }
        }
        self.bump_sample();
    }

    pub fn mouse_wheel(&mut self, x: i32, y: i32) {
        self.mouse.wheel_x = self.mouse.wheel_x.saturating_add(i64::from(x));
        self.mouse.wheel_y = self.mouse.wheel_y.saturating_add(i64::from(y));
        self.bump_sample();
    }

    pub fn pressed_keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.keyboard_usages.iter().copied()
    }

    fn bump_sample(&mut self) {
        self.sampling_number = self.sampling_number.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_sample_has_explicit_little_endian_fields_and_zero_padding() {
        let state = NpadState {
            connected: true,
            buttons: 0x0123_4567_89ab_cdef,
            left_stick: [-123, 456],
            right_stick: [i32::MIN, i32::MAX],
        };
        let sample = state.encode_common_sample(0x1020_3040_5060_7080);
        assert_eq!(&sample[..8], &0x1020_3040_5060_7080u64.to_le_bytes());
        assert_eq!(&sample[8..16], &state.buttons.to_le_bytes());
        for (index, axis) in [-123i32, 456, -32767, 32767].into_iter().enumerate() {
            let start = 16 + index * 4;
            assert_eq!(&sample[start..start + 4], &axis.to_le_bytes());
        }
        assert_eq!(&sample[32..36], &1u32.to_le_bytes());
        assert_eq!(&sample[36..], &[0; 4]);
        let disconnected = NpadState {
            connected: false,
            ..state
        }
        .encode_common_sample(9);
        assert_eq!(&disconnected[..8], &9u64.to_le_bytes());
        assert!(disconnected[8..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn controller_axes_are_bounded_and_disconnect_clears_controls() {
        let mut hid = HidState::default();
        hid.set_primary_npad(NpadState {
            connected: true,
            buttons: 3,
            left_stick: [i32::MIN, i32::MAX],
            right_stick: [2, -2],
        });
        assert_eq!(hid.primary_npad.left_stick, [-32767, 32767]);
        let sample = hid.sampling_number;
        hid.set_primary_npad(hid.primary_npad);
        assert_eq!(hid.sampling_number, sample);
        hid.set_primary_npad(NpadState {
            connected: false,
            ..hid.primary_npad
        });
        assert_eq!(hid.primary_npad, NpadState::default());
        assert_eq!(hid.sampling_number, sample + 1);
    }
}
