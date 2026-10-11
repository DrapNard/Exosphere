// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical keyboard controls for one virtual controller; no text/key logging.

use exosphere_horizon::hid::{HidState, NpadState};
use std::collections::HashSet;
use winit::{
    event::{ElementState, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
};

/// WASD: left stick; arrows: D-pad; L/K/I/J: A/B/X/Y; Q/E: L/R;
/// Z/C: ZL/ZR; Enter/Backspace: Plus/Minus. Physical codes are layout independent.
#[derive(Default)]
pub struct KeyboardController {
    pressed: HashSet<KeyCode>,
}

impl KeyboardController {
    /// Duplicate/repeat transitions do not change sampling. Focus loss releases
    /// every held control because the corresponding key-up may never arrive.
    pub fn event(&mut self, event: &WindowEvent, hid: &mut HidState) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.key(code, event.state == ElementState::Pressed, hid);
                }
            }
            WindowEvent::Focused(false) | WindowEvent::CloseRequested => {
                self.pressed.clear();
                hid.set_primary_npad(NpadState::default());
            }
            _ => {}
        }
    }

    fn key(&mut self, code: KeyCode, pressed: bool, hid: &mut HidState) {
        if button_bit(code).is_none()
            && !matches!(
                code,
                KeyCode::KeyW | KeyCode::KeyA | KeyCode::KeyS | KeyCode::KeyD
            )
        {
            return;
        }
        if pressed {
            self.pressed.insert(code);
        } else {
            self.pressed.remove(&code);
        }
        let buttons = self
            .pressed
            .iter()
            .filter_map(|code| button_bit(*code))
            .fold(0, |bits, bit| bits | (1u64 << bit));
        let axis = |positive, negative| {
            (i32::from(self.pressed.contains(&positive))
                - i32::from(self.pressed.contains(&negative)))
                * 32767
        };
        hid.set_primary_npad(NpadState {
            buttons,
            left_stick: [
                axis(KeyCode::KeyD, KeyCode::KeyA),
                axis(KeyCode::KeyW, KeyCode::KeyS),
            ],
            connected: true,
            ..NpadState::default()
        });
    }
}

// Horizon Npad button bit positions, not host key codes.
fn button_bit(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::KeyL => 0,
        KeyCode::KeyK => 1,
        KeyCode::KeyI => 2,
        KeyCode::KeyJ => 3,
        KeyCode::KeyQ => 6,
        KeyCode::KeyE => 7,
        KeyCode::KeyZ => 8,
        KeyCode::KeyC => 9,
        KeyCode::Enter => 10,
        KeyCode::Backspace => 11,
        KeyCode::ArrowLeft => 12,
        KeyCode::ArrowUp => 13,
        KeyCode::ArrowRight => 14,
        KeyCode::ArrowDown => 15,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_repeat_and_focus_loss() {
        let mut input = KeyboardController::default();
        let mut hid = HidState::default();
        input.key(KeyCode::KeyL, true, &mut hid);
        assert_eq!(hid.primary_npad.buttons, 1);
        let sample = hid.sampling_number;
        input.key(KeyCode::KeyL, true, &mut hid);
        assert_eq!(hid.sampling_number, sample);
        input.key(KeyCode::KeyK, true, &mut hid);
        assert_eq!(hid.primary_npad.buttons, 3);
        input.event(&WindowEvent::Focused(false), &mut hid);
        assert_eq!(hid.primary_npad, NpadState::default());
        input.key(KeyCode::KeyL, true, &mut hid);
        assert_eq!(hid.primary_npad.buttons, 1);
    }

    #[test]
    fn opposite_directions_cancel_without_losing_held_keys() {
        let mut input = KeyboardController::default();
        let mut hid = HidState::default();
        input.key(KeyCode::KeyW, true, &mut hid);
        assert_eq!(hid.primary_npad.left_stick, [0, 32767]);
        input.key(KeyCode::KeyS, true, &mut hid);
        assert_eq!(hid.primary_npad.left_stick, [0, 0]);
        input.key(KeyCode::KeyS, false, &mut hid);
        assert_eq!(hid.primary_npad.left_stick, [0, 32767]);
        input.key(KeyCode::ArrowLeft, true, &mut hid);
        assert_eq!(hid.primary_npad.buttons, 1 << 12);
    }
}
