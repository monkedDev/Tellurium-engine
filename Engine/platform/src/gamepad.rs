use crate::RawInputState;

#[cfg(windows)]
use crate::RawInputEvent;

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct State {
    buttons: u16,
    sticks: [i16; 4],
    triggers: [u8; 2],
}

#[derive(Default)]
pub struct Gamepads {
    #[cfg(windows)]
    previous: [Option<State>; 4],
}

impl Gamepads {
    #[cfg(windows)]
    pub fn poll(&mut self, input: &mut RawInputState) {
        use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

        for (index, previous) in self.previous.iter_mut().enumerate() {
            let id = index as u32;
            let mut raw = XINPUT_STATE::default();
            // SAFETY: XInput accepts indices 0 through 3 and writes to the supplied state.
            let current = if unsafe { XInputGetState(id, &mut raw) } == 0 {
                let pad = raw.Gamepad;
                Some(State {
                    buttons: pad.wButtons,
                    sticks: [pad.sThumbLX, pad.sThumbLY, pad.sThumbRX, pad.sThumbRY],
                    triggers: [pad.bLeftTrigger, pad.bRightTrigger],
                })
            } else {
                None
            };

            if *previous == current {
                continue;
            }

            if let Some(state) = current {
                if previous.is_none() {
                    input.push(RawInputEvent::GamepadConnected { id });
                }
                input.push(RawInputEvent::GamepadState {
                    id,
                    buttons: state.buttons,
                    sticks: state.sticks,
                    triggers: state.triggers,
                });
            } else {
                input.push(RawInputEvent::GamepadDisconnected { id });
            }
            *previous = current;
        }
    }

    #[cfg(not(windows))]
    pub fn poll(&mut self, _input: &mut RawInputState) {}
}
