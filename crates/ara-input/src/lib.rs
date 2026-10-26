//! Action maps shared by devices, agents, tests and replay; no platform dependency.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A local/session input owner; authority is assigned by the session adapter.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PlayerId(
    /// Numeric identity scoped to the owning session.
    pub u64,
);

/// The shape of an action's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// A held or released button.
    Button,
    /// One continuous axis.
    Axis,
    /// Two continuous axes, such as Move(x, y).
    Axis2,
}

/// Device-independent action data. Axes must be finite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActionValue {
    /// Button state; a one-tick true value is a press.
    Button(bool),
    /// One continuous axis.
    Axis(f32),
    /// Two continuous axes.
    Axis2([f32; 2]),
}

impl ActionValue {
    /// Returns the declaration that accepts this value.
    pub fn kind(self) -> ActionKind {
        match self {
            Self::Button(_) => ActionKind::Button,
            Self::Axis(_) => ActionKind::Axis,
            Self::Axis2(_) => ActionKind::Axis2,
        }
    }

    fn finite(self) -> bool {
        match self {
            Self::Button(_) => true,
            Self::Axis(v) => v.is_finite(),
            Self::Axis2(v) => v.into_iter().all(f32::is_finite),
        }
    }
}

/// One declared action, with device bindings supplied separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// Stable action name, for example Move or Jump.
    pub name: String,
    /// Value accepted by this action.
    pub kind: ActionKind,
}

/// A physical control's action contribution (devices use stable adapter-owned names).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// Device identity, such as keyboard or gamepad:0.
    pub device: String,
    /// Control identity, such as space or leftStick.
    pub control: String,
    /// Declared action name.
    pub action: String,
    /// The value emitted while this control is active.
    pub value: ActionValue,
}

/// Serializable declarations and physical bindings for one input context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionMap {
    actions: BTreeMap<String, ActionKind>,
    bindings: Vec<Binding>,
}

impl ActionMap {
    /// Declares a unique nonempty action name.
    pub fn declare(&mut self, action: Action) -> Result<(), String> {
        if action.name.is_empty() || self.actions.contains_key(&action.name) {
            return Err("action name is empty or already declared".into());
        }
        self.actions.insert(action.name, action.kind);
        Ok(())
    }

    /// Adds a binding after checking its declared action and value.
    pub fn bind(&mut self, binding: Binding) -> Result<(), String> {
        self.validate(&binding.action, binding.value)?;
        if binding.device.is_empty() || binding.control.is_empty() {
            return Err("device and control names must be nonempty".into());
        }
        self.bindings.push(binding);
        Ok(())
    }

    /// Checks an agent/device/replay action using the same declaration.
    pub fn validate(&self, action: &str, value: ActionValue) -> Result<(), String> {
        if self.actions.get(action) != Some(&value.kind()) || !value.finite() {
            return Err(format!("invalid value or unknown action `{action}`"));
        }
        Ok(())
    }

    /// Translates an active physical control to action injections in binding order.
    pub fn device(
        &self,
        player: PlayerId,
        device: &str,
        control: &str,
        hold_ticks: u32,
    ) -> Vec<Injection> {
        self.bindings
            .iter()
            .filter(|b| b.device == device && b.control == control)
            .map(|b| Injection {
                player,
                action: b.action.clone(),
                value: b.value,
                hold_ticks,
            })
            .collect()
    }

    /// Declarations sorted by stable action name.
    pub fn actions(&self) -> &BTreeMap<String, ActionKind> {
        &self.actions
    }
}

/// An action-level input, applied at a tick boundary; never a hardware event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Injection {
    /// Explicit owner, independent of a camera or local-player singleton.
    pub player: PlayerId,
    /// Declared action name.
    pub action: String,
    /// Action value.
    pub value: ActionValue,
    /// Duration in ticks; must be positive. Later injections replace the same action.
    pub hold_ticks: u32,
}

/// An applied input record suitable for replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRecord {
    /// Absolute tick, starting at one.
    pub tick: u64,
    /// Inputs in submission order; the last write to a player/action wins.
    pub inputs: Vec<Injection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Held {
    value: ActionValue,
    expires: u64,
}

/// Restorable action state. Missing inputs are neutral after their hold expires.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputState {
    // A nested map keeps the replay/snapshot representation valid JSON.
    players: BTreeMap<PlayerId, BTreeMap<String, Held>>,
}

impl InputState {
    /// Active fixed-tick actions in player/name order, for runtime language adapters.
    pub fn iter(&self) -> impl Iterator<Item = (PlayerId, &str, ActionValue)> {
        self.players.iter().flat_map(|(player, actions)| {
            actions.iter().map(move |(name, held)| (*player, name.as_str(), held.value))
        })
    }
    /// Validates a complete batch before changing anything, then applies at `tick`.
    pub fn apply(
        &mut self,
        map: &ActionMap,
        tick: u64,
        inputs: &[Injection],
    ) -> Result<(), String> {
        for input in inputs {
            map.validate(&input.action, input.value)?;
            if input.hold_ticks == 0 || tick.checked_add(u64::from(input.hold_ticks)).is_none() {
                return Err("invalid input hold duration".into());
            }
        }
        for actions in self.players.values_mut() {
            actions.retain(|_, held| held.expires > tick);
        }
        self.players.retain(|_, actions| !actions.is_empty());
        for input in inputs {
            self.players.entry(input.player).or_default().insert(
                input.action.clone(),
                Held {
                    value: input.value,
                    expires: tick + u64::from(input.hold_ticks),
                },
            );
        }
        Ok(())
    }

    /// The active action value, or None for neutral/unbound input.
    pub fn get(&self, player: PlayerId, action: &str) -> Option<ActionValue> {
        self.players
            .get(&player)?
            .get(action)
            .map(|held| held.value)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn devices_and_injection_share_validation_and_hold_semantics() {
        let mut map = ActionMap::default();
        map.declare(Action {
            name: "Move".into(),
            kind: ActionKind::Axis2,
        })
        .unwrap();
        map.bind(Binding {
            device: "keyboard".into(),
            control: "d".into(),
            action: "Move".into(),
            value: ActionValue::Axis2([1., 0.]),
        })
        .unwrap();
        let inputs = map.device(PlayerId(7), "keyboard", "d", 2);
        let mut state = InputState::default();
        state.apply(&map, 1, &inputs).unwrap();
        state.apply(&map, 2, &[]).unwrap();
        assert_eq!(
            state.get(PlayerId(7), "Move"),
            Some(ActionValue::Axis2([1., 0.]))
        );
        state.apply(&map, 3, &[]).unwrap();
        assert_eq!(state.get(PlayerId(7), "Move"), None);
        assert!(
            map.validate("Move", ActionValue::Axis2([f32::NAN, 0.]))
                .is_err()
        );
        assert!(map.validate("Move", ActionValue::Button(true)).is_err());
    }
}
