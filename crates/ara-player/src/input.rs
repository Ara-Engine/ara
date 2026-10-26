//! Device input → action injections, one mapping for every play client: windowed
//! players feed it SDL events, embedded players the raw events their owner forwards
//! (`play.device`). Bindings: W A S D and the arrows Move, Space Jump (once per
//! press), Left Shift Sprint, right-drag Look. Injections go through
//! `Session::inject`, the replayable path agents use too.
use ara_input::{ActionValue, Injection, PlayerId};
use ara_runtime::control::{JUMP, LOOK, MOVE, SPRINT, INTERACT, INVENTORY};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// SDL scancodes used by the default bindings.
const W: i32 = 26;
const A: i32 = 4;
const S: i32 = 22;
const D: i32 = 7;
const UP: i32 = 82;
const DOWN: i32 = 81;
const LEFT: i32 = 80;
const RIGHT: i32 = 79;
const SPACE: i32 = 44;
const LEFT_SHIFT: i32 = 225;
/// The SDL right mouse button: hold it to look.
const RIGHT_BUTTON: u8 = 3;
/// Holds that last until replaced (the next change re-injects).
const HELD: u32 = 1 << 30;

/// One raw device event of a play client. The JSON form (`play.device` events,
/// mirrored by `ara_play_host::embed::DeviceEvent`) is
/// `{"kind": "key", "scancode": 26, "down": true}`, `{"kind": "mouseButton",
/// "button": 3, "down": true}`, `{"kind": "mouseMotion", "dx": 4, "dy": -2}` or
/// `{"kind": "focusLost"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DeviceEvent {
    /// A key changed state (SDL scancode). A press of a held key (OS repeat) is
    /// ignored.
    Key {
        /// SDL physical key scancode.
        scancode: i32,
        /// Pressed (true) or released.
        down: bool,
    },
    /// A mouse button changed state (SDL numbering: 1 left, 2 middle, 3 right).
    MouseButton {
        /// SDL button number.
        button: u8,
        /// Pressed (true) or released.
        down: bool,
    },
    /// Relative mouse motion in pixels.
    MouseMotion {
        /// Rightward motion.
        dx: f32,
        /// Downward motion.
        dy: f32,
    },
    /// The client lost focus: every held key and button is released and look stops.
    FocusLost,
}

/// Held device state and what was last injected, per client.
#[derive(Debug, Default)]
pub struct Mapper {
    keys: BTreeSet<i32>,
    orbit: bool,
    look: [f32; 2],
    jump: bool,
    interact: bool,
    inventory: bool,
    sent_move: [f32; 2],
    sent_sprint: bool,
}

impl Mapper {
    /// Folds one event into the held state; [`Mapper::injections`] turns it into
    /// actions for the next tick.
    pub fn apply(&mut self, event: DeviceEvent) {
        match event {
            DeviceEvent::Key {
                scancode,
                down: true,
            } => {
                if self.keys.insert(scancode) {
                    self.jump |= scancode == SPACE;
                    self.interact |= scancode == 8;
                    self.inventory |= scancode == 43;
                }
            }
            DeviceEvent::Key {
                scancode,
                down: false,
            } => {
                self.keys.remove(&scancode);
            }
            DeviceEvent::MouseButton {
                button: RIGHT_BUTTON,
                down,
            } => self.orbit = down,
            DeviceEvent::MouseButton { .. } => {}
            DeviceEvent::MouseMotion { dx, dy } => {
                if self.orbit && dx.is_finite() && dy.is_finite() {
                    self.look[0] += dx * 0.15;
                    self.look[1] += dy * 0.1;
                }
            }
            DeviceEvent::FocusLost => {
                self.keys.clear();
                self.orbit = false;
                self.look = [0.0; 2];
                self.jump = false;
                self.interact = false;
                self.inventory = false;
            }
        }
    }

    /// The injections for `player` that changed since the last call: Move and
    /// Sprint when their held value changed, Jump once per press, Look per motion.
    pub fn injections(&mut self, player: PlayerId) -> Vec<Injection> {
        let held = |a: i32, b: i32| self.keys.contains(&a) || self.keys.contains(&b);
        let axis = |pos: bool, neg: bool| f32::from(u8::from(pos)) - f32::from(u8::from(neg));
        let movement = [
            axis(held(D, RIGHT), held(A, LEFT)),
            axis(held(W, UP), held(S, DOWN)),
        ];
        let sprint = self.keys.contains(&LEFT_SHIFT);
        let mut inputs = Vec::new();
        if movement != self.sent_move {
            inputs.push(Injection {
                player,
                action: MOVE.into(),
                value: ActionValue::Axis2(movement),
                hold_ticks: HELD,
            });
            self.sent_move = movement;
        }
        if sprint != self.sent_sprint {
            inputs.push(Injection {
                player,
                action: SPRINT.into(),
                value: ActionValue::Button(sprint),
                hold_ticks: HELD,
            });
            self.sent_sprint = sprint;
        }
        if std::mem::take(&mut self.jump) {
            inputs.push(Injection {
                player,
                action: JUMP.into(),
                value: ActionValue::Button(true),
                hold_ticks: 1,
            });
        }
        if self.look != [0.0; 2] {
            let look = std::mem::take(&mut self.look);
            inputs.push(Injection {
                player,
                action: LOOK.into(),
                value: ActionValue::Axis2(look),
                hold_ticks: 1,
            });
        }
        for (action, pressed) in [(INTERACT, &mut self.interact), (INVENTORY, &mut self.inventory)] {
            if std::mem::take(pressed) {
                inputs.push(Injection { player, action: action.into(), value: ActionValue::Button(true), hold_ticks: 1 });
            }
        }
        inputs
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn key(scancode: i32, down: bool) -> DeviceEvent {
        DeviceEvent::Key { scancode, down }
    }

    fn actions(inputs: &[Injection]) -> Vec<(String, ActionValue)> {
        inputs.iter().map(|i| (i.action.clone(), i.value)).collect()
    }

    #[test]
    fn keys_map_to_held_move_and_sprint_and_one_jump_per_press() {
        let player = PlayerId(1);
        let mut mapper = Mapper::default();
        mapper.apply(key(W, true));
        mapper.apply(key(D, true));
        mapper.apply(key(LEFT_SHIFT, true));
        mapper.apply(key(SPACE, true));
        let inputs = mapper.injections(player);
        assert_eq!(
            actions(&inputs),
            vec![
                (MOVE.to_string(), ActionValue::Axis2([1.0, 1.0])),
                (SPRINT.to_string(), ActionValue::Button(true)),
                (JUMP.to_string(), ActionValue::Button(true)),
            ]
        );
        assert!(inputs.iter().all(|i| i.player == player));
        assert_eq!(inputs[0].hold_ticks, HELD);
        assert_eq!(inputs[2].hold_ticks, 1);
        // Nothing changed: nothing to inject; an OS repeat of Space is no new jump.
        mapper.apply(key(SPACE, true));
        assert!(mapper.injections(player).is_empty());
        mapper.apply(key(W, false));
        mapper.apply(key(DOWN, true));
        assert_eq!(
            actions(&mapper.injections(player)),
            vec![(MOVE.to_string(), ActionValue::Axis2([1.0, -1.0]))]
        );
    }

    #[test]
    fn right_drag_looks_and_focus_loss_releases_everything() {
        let player = PlayerId(2);
        let mut mapper = Mapper::default();
        mapper.apply(DeviceEvent::MouseMotion { dx: 10.0, dy: 0.0 });
        assert!(
            mapper.injections(player).is_empty(),
            "no look without the button"
        );
        mapper.apply(DeviceEvent::MouseButton {
            button: RIGHT_BUTTON,
            down: true,
        });
        mapper.apply(DeviceEvent::MouseMotion {
            dx: 10.0,
            dy: -20.0,
        });
        assert_eq!(
            actions(&mapper.injections(player)),
            vec![(LOOK.to_string(), ActionValue::Axis2([1.5, -2.0]))]
        );
        mapper.apply(key(A, true));
        mapper.apply(key(LEFT_SHIFT, true));
        mapper.injections(player);
        mapper.apply(DeviceEvent::MouseMotion { dx: 4.0, dy: 4.0 });
        mapper.apply(DeviceEvent::FocusLost);
        assert_eq!(
            actions(&mapper.injections(player)),
            vec![
                (MOVE.to_string(), ActionValue::Axis2([0.0, 0.0])),
                (SPRINT.to_string(), ActionValue::Button(false)),
            ],
            "held keys released, pending look dropped"
        );
        mapper.apply(DeviceEvent::MouseMotion { dx: 4.0, dy: 4.0 });
        assert!(mapper.injections(player).is_empty(), "look stopped");
    }

    /// The owner's typed events (ara-play-host) arrive here as the same JSON.
    #[test]
    fn owner_events_parse_as_player_events() {
        use ara_play_host::embed::DeviceEvent as Owner;
        let events = [
            Owner::Key {
                scancode: 26,
                down: true,
            },
            Owner::MouseButton {
                button: 3,
                down: false,
            },
            Owner::MouseMotion { dx: 1.5, dy: -2.0 },
            Owner::FocusLost,
        ];
        let parsed: Vec<DeviceEvent> =
            serde_json::from_value(serde_json::to_value(events).unwrap()).unwrap();
        assert_eq!(
            parsed,
            vec![
                key(26, true),
                DeviceEvent::MouseButton {
                    button: 3,
                    down: false
                },
                DeviceEvent::MouseMotion { dx: 1.5, dy: -2.0 },
                DeviceEvent::FocusLost,
            ]
        );
        assert_eq!(
            crate::embed::method::DEVICE,
            ara_play_host::embed::method::DEVICE
        );
        assert_eq!(
            crate::embed::method::EMBED,
            ara_play_host::embed::method::EMBED
        );
        assert_eq!(
            crate::embed::method::FRAME,
            ara_play_host::embed::method::FRAME
        );
        assert_eq!(
            crate::embed::method::RELEASE,
            ara_play_host::embed::method::RELEASE
        );
        assert_eq!(
            crate::embed::method::RESIZE,
            ara_play_host::embed::method::RESIZE
        );
        assert_eq!(
            crate::embed::method::SURFACE,
            ara_play_host::embed::method::SURFACE
        );
    }
}
