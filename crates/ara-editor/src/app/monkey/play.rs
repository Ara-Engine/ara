//! Bound only this run's Play worlds with explicit, journaled protocol actions.
use super::{Action, Editor};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Default, PartialEq, Eq)]
pub(super) struct Inventory {
    known: BTreeSet<String>,
    active: BTreeSet<String>,
    stopping: BTreeSet<String>,
    players: usize,
}
impl Inventory {
    fn from_value(value: &Value) -> Self {
        let mut inventory = Self::default();
        for world in value["worlds"].as_array().into_iter().flatten() {
            let Some(id) = world["world"].as_str() else {
                continue;
            };
            inventory.known.insert(id.into());
            if world["status"] == "stopping" {
                inventory.stopping.insert(id.into());
            } else {
                inventory.active.insert(id.into());
            }
            inventory.players += world["players"].as_array().map_or(0, Vec::len);
        }
        for world in value["stopping"].as_array().into_iter().flatten() {
            if let Some(id) = world["world"].as_str() {
                inventory.known.insert(id.into());
                inventory.stopping.insert(id.into());
            }
        }
        inventory
    }
    fn read(editor: &mut Editor) -> ara_ui::Result<Self> {
        Ok(Self::from_value(&editor.action("play.status", json!({}))?))
    }
}

#[derive(Default)]
pub(super) struct State {
    initialized: bool,
    baseline: BTreeSet<String>,
    owned: BTreeSet<String>,
    current: Inventory,
    deferred: Option<Action>,
    peak_owned: usize,
    peak_players: usize,
    cleanup_stops: usize,
    cleanup_polls: usize,
}
impl State {
    /// Defer repeated starts until this run's older worlds have been retired.
    /// Every stop/status is an ordinary action, so exact replay needs no inference.
    pub fn next(&mut self, next: impl FnOnce() -> Action) -> Action {
        let action = self.deferred.take().unwrap_or_else(next);
        if starts(&action)
            && let Some(cleanup) = self.cleanup()
        {
            self.deferred = Some(action);
            return cleanup;
        }
        action
    }
    pub fn drain(&mut self) -> Option<Action> {
        self.deferred = None;
        self.cleanup()
    }
    fn cleanup(&mut self) -> Option<Action> {
        if let Some(id) = self.current.active.intersection(&self.owned).next() {
            self.cleanup_stops += 1;
            return Some(Action::Protocol {
                method: "play.stop".into(),
                params: json!({"world":id}),
                raw: false,
            });
        }
        if !self.drained() {
            self.cleanup_polls += 1;
            return Some(Action::Protocol {
                method: "play.status".into(),
                params: json!({}),
                raw: false,
            });
        }
        None
    }
    pub fn drained(&self) -> bool {
        self.current
            .known
            .intersection(&self.owned)
            .next()
            .is_none()
    }
    pub fn cleanup_actions(&self) -> u64 {
        (self.cleanup_stops + self.cleanup_polls) as u64
    }
    pub fn before(&mut self, editor: &mut Editor) -> ara_ui::Result<Inventory> {
        let inventory = Inventory::read(editor)?;
        if !self.initialized {
            self.baseline = inventory.known.clone();
            self.initialized = true;
        }
        Ok(inventory)
    }
    pub fn after(
        &mut self,
        editor: &mut Editor,
        before: &Inventory,
    ) -> ara_ui::Result<Option<Value>> {
        let after = Inventory::read(editor)?;
        Ok(self.record(before, after))
    }
    fn record(&mut self, before: &Inventory, after: Inventory) -> Option<Value> {
        // Authority is single-threaded during the callback: another connection's
        // queued requests do not run between these two status reads.
        self.owned.extend(
            after
                .known
                .difference(&before.known)
                .filter(|id| !self.baseline.contains(*id))
                .cloned(),
        );
        self.peak_owned = self
            .peak_owned
            .max(after.known.intersection(&self.owned).count());
        self.peak_players = self.peak_players.max(after.players);
        let changed = &after != before;
        self.current = after;
        changed.then(|| self.report())
    }
    pub fn report(&self) -> Value {
        json!({"baselineWorlds":self.baseline,"ownedWorldsCreated":self.owned,"peakOwnedWorlds":self.peak_owned,
            "peakRegisteredPlayers":self.peak_players,"ownedActive":self.current.active.intersection(&self.owned).collect::<Vec<_>>(),
            "ownedStopping":self.current.stopping.intersection(&self.owned).collect::<Vec<_>>(),
            "recordedCleanupStops":self.cleanup_stops,"recordedCleanupPolls":self.cleanup_polls,"deferredStart":self.deferred.is_some(),
            "note":"Player counts cover registered worlds; pooled players and in-flight preparation children remain owned by PlayService."})
    }
}
fn starts(action: &Action) -> bool {
    match action {
        Action::Command { id, .. } => id == "play.start",
        Action::Protocol {
            method,
            params,
            raw: false,
        } => matches!(method.as_str(), "play.start" | "editor.play") && params.is_object(),
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::shell::commands::Target;
    #[test]
    fn repeated_start_cleanup_never_targets_baseline_worlds_and_waits_nonblocking() {
        let mut state = State {
            initialized: true,
            baseline: BTreeSet::from(["play:1".into()]),
            owned: BTreeSet::from(["play:2".into()]),
            current: Inventory::from_value(
                &json!({"worlds":[{"world":"play:1"},{"world":"play:2"}]}),
            ),
            ..State::default()
        };
        let next = || Action::Command {
            id: "play.start".into(),
            target: Target::None,
        };
        let stopped = serde_json::to_value(state.next(next)).unwrap();
        assert_eq!(stopped["method"], "play.stop");
        assert_eq!(stopped["params"]["world"], "play:2");
        state.current = Inventory::from_value(
            &json!({"worlds":[{"world":"play:1"}],"stopping":[{"world":"play:2"}]}),
        );
        let polled = serde_json::to_value(
            state.next(|| panic!("deferred start must retain its exact action")),
        )
        .unwrap();
        assert_eq!(polled["method"], "play.status");
        state.current = Inventory::from_value(&json!({"worlds":[{"world":"play:1"}]}));
        let started = serde_json::to_value(
            state.next(|| panic!("deferred start must retain its exact action")),
        )
        .unwrap();
        assert_eq!(started["kind"], "command");
        assert_eq!(started["id"], "play.start");
    }

    #[test]
    fn only_worlds_created_inside_the_callback_become_owned() {
        let mut state = State {
            initialized: true,
            baseline: BTreeSet::from(["play:1".into()]),
            ..State::default()
        };
        let before =
            Inventory::from_value(&json!({"worlds":[{"world":"play:1"},{"world":"play:3"}]}));
        let after = Inventory::from_value(
            &json!({"worlds":[{"world":"play:1"},{"world":"play:2"},{"world":"play:3"}]}),
        );
        let _ = state.record(&before, after);
        assert_eq!(state.owned, BTreeSet::from(["play:2".into()]));
        let cancelled = Inventory::from_value(
            &json!({"worlds":[{"world":"play:1"},{"world":"play:2","status":"stopping"},{"world":"play:3"}]}),
        );
        assert!(cancelled.stopping.contains("play:2"));
        assert!(!cancelled.active.contains("play:2"));
    }
}
