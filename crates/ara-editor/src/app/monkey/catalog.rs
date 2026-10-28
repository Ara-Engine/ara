//! Coverage follows loaded native and C# registrations, including late generations.
use super::{Action, actions};
use crate::{extensions::Catalog, shell::commands};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Registration {
    id: String,
    builtin: bool,
    generation: Option<String>,
}
struct Snapshot {
    registrations: BTreeMap<String, Registration>,
    generation: Option<String>,
    dynamic: BTreeSet<String>,
}
impl Snapshot {
    fn matches(&self, catalog: &Catalog) -> bool {
        self.generation == catalog.generation
            && self.dynamic.len() == catalog.commands.len()
            && catalog
                .commands
                .iter()
                .all(|c| self.dynamic.contains(&c.id))
    }
}
struct Probe {
    registration: Registration,
    case: usize,
}
#[derive(PartialEq, Eq)]
struct Final {
    ids: BTreeSet<String>,
    generation: Option<String>,
}

#[derive(Default)]
pub(super) struct State {
    initial: Option<BTreeSet<String>>,
    latest: Option<Snapshot>,
    known: BTreeMap<Registration, BTreeSet<usize>>,
    pending: VecDeque<Probe>,
    issued: Option<Probe>,
    final_catalog: Option<Final>,
    settled: bool,
    errors: BTreeSet<String>,
    changes: Vec<Value>,
    changes_omitted: u64,
    events: Vec<Value>,
    skipped: u64,
}
impl State {
    /// Native table order stays fixed; newly loaded C# IDs are sorted before queuing.
    /// Normal frames only compare borrowed IDs/generation, without cloning catalogs.
    pub fn observe(&mut self, catalog: &Catalog) {
        if self.latest.as_ref().is_some_and(|old| old.matches(catalog)) {
            return;
        }
        if !catalog.commands.is_empty() && catalog.generation.as_deref().is_none_or(str::is_empty) {
            self.errors
                .insert("Loaded C# Shell registrations have no generation".into());
        }
        let mut entries: Vec<_> = commands::entries(catalog).collect();
        let builtin_count = commands::all().len();
        entries[builtin_count..].sort_by(|a, b| a.id.cmp(b.id));
        let mut registrations = BTreeMap::new();
        let mut dynamic = BTreeSet::new();
        for entry in entries {
            let registration = Registration {
                id: entry.id.to_owned(),
                builtin: entry.builtin.is_some(),
                generation: if entry.builtin.is_some() {
                    None
                } else {
                    catalog.generation.clone()
                },
            };
            if !registration.builtin {
                dynamic.insert(registration.id.clone());
            }
            if registration.id.is_empty()
                || registrations
                    .insert(registration.id.clone(), registration.clone())
                    .is_some()
            {
                self.errors.insert(format!(
                    "Duplicate or empty Shell registration: {}",
                    registration.id
                ));
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.known.entry(registration.clone())
            {
                entry.insert(BTreeSet::new());
                self.pending
                    .extend((0..actions::edge_case_count()).map(|case| Probe {
                        registration: registration.clone(),
                        case,
                    }));
            }
        }
        let ids: BTreeSet<_> = registrations.keys().cloned().collect();
        let previous: BTreeSet<_> = self
            .latest
            .as_ref()
            .map(|old| old.registrations.keys().cloned().collect())
            .unwrap_or_default();
        let event = json!({"phase":"shellCatalog","initial":self.initial.is_none(),
            "generation":catalog.generation,"ids":ids,
            "added":ids.difference(&previous).collect::<Vec<_>>(),
            "removed":previous.difference(&ids).collect::<Vec<_>>()});
        if self.initial.is_some() {
            if self.changes.len() < 64 {
                self.changes.push(event.clone());
            } else {
                self.changes_omitted += 1;
            }
        } else {
            self.initial = Some(ids);
        }
        self.events.push(event);
        self.latest = Some(Snapshot {
            registrations,
            generation: catalog.generation.clone(),
            dynamic,
        });
        self.settled = false;
    }

    pub fn pending(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn next(&mut self) -> Option<Action> {
        let probe = self.pending.pop_front()?;
        let action = Action::Command {
            id: probe.registration.id.clone(),
            target: actions::edge_target(probe.case),
        };
        self.issued = Some(probe);
        Some(action)
    }
    pub fn len(&self) -> usize {
        self.latest.as_ref().map_or(0, |s| s.registrations.len())
    }
    pub fn id(&self, index: usize) -> Option<&str> {
        self.latest
            .as_ref()?
            .registrations
            .keys()
            .nth(index)
            .map(String::as_str)
    }
    /// A removed/replaced queued registration is still offered to the real Shell,
    /// but cannot receive coverage credit for a different loaded implementation.
    pub fn record(&mut self, id: &str, case: usize) {
        let current = self.latest.as_ref().and_then(|s| s.registrations.get(id));
        let issued = self
            .issued
            .take()
            .filter(|p| p.registration.id == id && p.case == case);
        if let Some(probe) = &issued
            && current != Some(&probe.registration)
        {
            self.skipped += 1;
            self.events.push(json!({"phase":"shellRegistrationUnavailable","registration":probe.registration,"case":case,"loaded":current}));
            return;
        }
        if let Some(registration) = current
            && let Some(cases) = self.known.get_mut(registration)
        {
            cases.insert(case);
        }
    }
    /// The authority can publish a generation before Editor's queued controls are
    /// consumed. A mismatch keeps the soak alive until its ordinary next frame.
    pub fn reconcile_final(&mut self, response: &Value, settled: bool) {
        let previously_settled = self.settled;
        self.settled = false;
        let Some(commands) = response["commands"].as_array() else {
            self.errors
                .insert("Final editor.commands returned no command catalog".into());
            return;
        };
        let mut ids = BTreeSet::new();
        for command in commands {
            let Some(id) = command["id"].as_str().filter(|id| !id.is_empty()) else {
                self.errors
                    .insert("Final editor.commands contains an empty or invalid ID".into());
                return;
            };
            if !ids.insert(id.to_owned()) {
                self.errors
                    .insert(format!("Final editor.commands repeats {id}"));
            }
        }
        let generation = match &response["generation"] {
            Value::Null => None,
            Value::String(value) => Some(value.clone()),
            _ => {
                self.errors
                    .insert("Final editor.commands has an invalid generation".into());
                return;
            }
        };
        let final_catalog = Final { ids, generation };
        let changed =
            self.final_catalog.as_ref() != Some(&final_catalog) || previously_settled != settled;
        self.final_catalog = Some(final_catalog);
        self.settled = settled;
        if changed {
            self.events.push(json!({"phase":"shellCatalogFinal","ids":self.final_catalog.as_ref().map(|c| &c.ids),
                "generation":self.final_catalog.as_ref().and_then(|c| c.generation.as_deref()),
                "matched":self.final_matched(),"settled":settled}));
        }
    }
    pub fn final_matched(&self) -> bool {
        match (&self.latest, &self.final_catalog) {
            (Some(loaded), Some(authority)) => {
                loaded.generation == authority.generation
                    && loaded.registrations.keys().eq(authority.ids.iter())
            }
            _ => false,
        }
    }
    pub fn drain_pending(&self) -> bool {
        self.pending() || (self.errors.is_empty() && !self.final_matched())
    }
    pub fn complete(&self) -> bool {
        self.settled
            && self.final_matched()
            && self.errors.is_empty()
            && !self.known.is_empty()
            && self.missing().is_empty()
    }
    pub fn missing(&self) -> Vec<Value> {
        self.known
            .iter()
            .filter_map(|(registration, seen)| {
                let missing: Vec<_> = (0..actions::edge_case_count())
                    .filter(|case| !seen.contains(case))
                    .collect();
                (!missing.is_empty()).then(|| {
                    json!({"command":registration.id,"builtin":registration.builtin,
                "generation":registration.generation,"cases":missing})
                })
            })
            .collect()
    }
    pub fn ids(&self) -> BTreeSet<&str> {
        self.known.keys().map(|r| r.id.as_str()).collect()
    }
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }
    pub fn report(&self) -> Value {
        let unloaded: Vec<_> = self
            .final_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .ids
                    .iter()
                    .filter(|id| {
                        self.latest
                            .as_ref()
                            .is_none_or(|loaded| !loaded.registrations.contains_key(*id))
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({"shellCommandCatalogInitial":self.initial,"shellCommandCatalogObserved":self.known.keys().collect::<Vec<_>>(),
            "shellCommandCatalogFinal":self.final_catalog.as_ref().map(|c| &c.ids),
            "shellCommandCatalogFinalGeneration":self.final_catalog.as_ref().and_then(|c| c.generation.as_deref()),
            "shellCommandCatalogFinalMatched":self.final_matched(),"shellCommandCatalogSettled":self.settled,
            "shellCommandCatalogErrors":self.errors,"shellCommandCatalogChanges":self.changes,
            "shellCommandCatalogFinalUnloaded":unloaded,
            "shellCommandCatalogChangesOmitted":self.changes_omitted,"shellRegistrationProbesSkipped":self.skipped,
            "shellRegistrationCases":self.known.iter().map(|(r,cases)| json!({"registration":r,"cases":cases})).collect::<Vec<_>>(),
            "shellCommandCoverageComplete":self.complete(),
            "shellCommandCoverageScope":"All observed native and loaded C# editor registrations, per generation; attempts can reject before callback execution."})
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
