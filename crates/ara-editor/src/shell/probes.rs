//! Last-good probe projection and human-owned derived jobs; no renderer or file I/O.
use super::*;
use ara_runtime_render::probes::{ProbeRow, ProbeSnapshot};
use std::time::{Duration, Instant};
#[cfg(test)]
mod tests;
mod view;
pub(super) const CSS: &str = include_str!("probes/probes.css");
const CADENCE: Duration = Duration::from_millis(250);
const TERMINAL_HISTORY: usize = 32;

struct Job {
    doc: String,
    entities: Vec<String>,
    latest: Option<Value>,
    error: Option<String>,
}
impl Job {
    fn running(&self) -> bool {
        self.latest
            .as_ref()
            .is_none_or(|value| value["status"] == "running")
    }
    fn cancellable(&self) -> bool {
        self.error.is_none()
            && self
                .latest
                .as_ref()
                .is_some_and(|value| value["status"] == "running" && value["cancellable"] == true)
    }
}

/// Shared across surfaces; an empty since response never erases the last full rows.
#[derive(Default)]
pub(crate) struct State {
    doc: String,
    snapshot: Option<ProbeSnapshot>,
    capabilities: Option<ara_runtime_render::probes::RenderCapabilities>,
    version: u64,
    available: bool,
    error: Option<String>,
    next_poll: Option<Instant>,
    jobs: BTreeMap<String, Job>,
    job_order: std::collections::VecDeque<String>,
}
impl State {
    fn current_doc(&mut self, doc: &str) -> bool {
        if self.doc == doc {
            return false;
        }
        self.doc.clear();
        self.doc.push_str(doc);
        self.snapshot = None;
        self.capabilities = None;
        self.version = 0;
        self.available = false;
        self.error = None;
        self.next_poll = None;
        true
    }
    fn observe(&mut self, snapshot: ProbeSnapshot, since: Option<u64>) -> bool {
        let available = snapshot.version != 0;
        let full = since.is_none() || snapshot.version != self.version;
        let changed = self.available != available
            || (available && self.version != snapshot.version)
            || self.capabilities.as_ref() != Some(&snapshot.capabilities)
            || self.error.is_some();
        self.capabilities = Some(snapshot.capabilities.clone());
        self.available = available;
        self.error = None;
        if available {
            self.version = snapshot.version;
            if full {
                self.snapshot = Some(snapshot);
            }
        }
        changed
    }
    fn reason(&self) -> Option<&str> {
        if let Some(error) = self.error.as_deref() {
            return Some(error);
        }
        if self.available
            && self
                .capabilities
                .as_ref()
                .is_some_and(|caps| caps.reflection_probes)
        {
            return None;
        }
        self.capabilities
            .as_ref()
            .and_then(|caps| caps.reasons.get("reflectionProbes"))
            .map(String::as_str)
            .or(Some(
                "Waiting for the current Scene renderer's probe capabilities",
            ))
    }
    fn row(&self, entity: &str) -> Option<&ProbeRow> {
        self.snapshot
            .as_ref()?
            .probes
            .iter()
            .find(|row| row.doc == self.doc && row.entity == entity)
    }
    fn trim_terminal(&mut self) {
        while self.jobs.values().filter(|job| !job.running()).count() > TERMINAL_HISTORY {
            let Some(index) = self
                .job_order
                .iter()
                .position(|id| self.jobs.get(id).is_none_or(|job| !job.running()))
            else {
                break;
            };
            if let Some(id) = self.job_order.remove(index) {
                self.jobs.remove(&id);
            }
        }
    }
}

impl Shell {
    /// Shared registry/menu disabled reason; querying a command never touches the renderer.
    pub(crate) fn probe_capture_blocker(
        &self,
        editor: &Editor,
        target: &commands::Target,
    ) -> Option<&str> {
        if self.probe_ui.doc != editor.state.doc {
            return Some("Waiting for this Scene's probe snapshot");
        }
        if let Some(reason) = self.probe_ui.reason() {
            return Some(reason);
        }
        let selected = if let commands::Target::Entity(entity) = target {
            self.probe_ui.row(entity).is_some()
        } else {
            editor
                .state
                .selection
                .iter()
                .any(|entity| self.probe_ui.row(entity).is_some())
        };
        (!selected).then_some("Select a Reflection Probe reported by the current Scene renderer")
    }
    pub(crate) fn probe_cancel_blocker(
        &self,
        editor: &Editor,
        target: &commands::Target,
    ) -> Option<&str> {
        let eligible = self.probe_ui.jobs.iter().any(|(id, job)| {
            job.cancellable()
                && match target {
                    commands::Target::Text(wanted) => wanted == id,
                    commands::Target::Entity(entity) => {
                        job.doc == editor.state.doc && job.entities.contains(entity)
                    }
                    _ => {
                        job.doc == editor.state.doc
                            && job
                                .entities
                                .iter()
                                .any(|entity| editor.state.selection.contains(entity))
                    }
                }
        });
        (!eligible)
            .then_some("No cancellable capture request from this editor session matches this probe")
    }

    /// Bounded read-only polling; root calls once on the main surface after Host tick.
    pub(crate) fn poll_probes(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let mut changed = self.probe_ui.current_doc(&editor.state.doc);
        let now = Instant::now();
        if self.probe_ui.next_poll.is_some_and(|next| now < next) {
            return Ok(changed);
        }
        self.probe_ui.next_poll = Some(now + CADENCE);
        let since = self
            .probe_ui
            .snapshot
            .as_ref()
            .map(|_| self.probe_ui.version);
        match editor.action(
            "render.probes.query",
            json!({"doc":editor.state.doc,"since":since}),
        ) {
            Ok(value) => match serde_json::from_value::<ProbeSnapshot>(value) {
                Ok(snapshot) => {
                    if snapshot
                        .probes
                        .iter()
                        .any(|row| row.doc != editor.state.doc)
                    {
                        changed |= self.probe_ui.error.as_deref()
                            != Some("Probe snapshot did not belong to the current Scene");
                        self.probe_ui.error =
                            Some("Probe snapshot did not belong to the current Scene".into());
                        self.probe_ui.available = false;
                    } else {
                        changed |= self.probe_ui.observe(snapshot, since);
                    }
                }
                Err(error) => {
                    let message =
                        format!("The renderer's probe response could not be read: {error}");
                    changed |= self.probe_ui.error.as_ref() != Some(&message);
                    self.probe_ui.error = Some(message);
                    self.probe_ui.available = false;
                }
            },
            Err(error) => {
                let message = error.to_string();
                changed |= self.probe_ui.error.as_ref() != Some(&message);
                self.probe_ui.error = Some(message);
                self.probe_ui.available = false;
            }
        }
        for (id, job) in &mut self.probe_ui.jobs {
            if !job.running() {
                continue;
            }
            match editor.action("jobs.status", json!({"job":id})) {
                Ok(status) => {
                    changed |= job.latest.as_ref() != Some(&status) || job.error.is_some();
                    job.latest = Some(status);
                    job.error = None;
                }
                Err(error) => {
                    let message = format!(
                        "Probe job status unavailable; retirement is not established: {error}"
                    );
                    changed |= job.error.as_ref() != Some(&message);
                    job.error = Some(message);
                }
            }
        }
        self.probe_ui.trim_terminal();
        Ok(changed)
    }

    pub(crate) fn refresh_probe_state(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.probe_ui.next_poll = None;
        self.poll_probes(editor)?;
        self.rebuild(editor)
    }

    /// Explicit derived work preserves camera, selection and authored documents.
    pub(crate) fn recapture_probes(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        self.probe_ui.current_doc(&editor.state.doc);
        if let Some(reason) = self.probe_ui.reason() {
            return Err(reason.to_owned().into());
        }
        let resolved = self.resolved(editor)?;
        let entities: Vec<_> = if let commands::Target::Entity(entity) = target {
            vec![entity.clone()]
        } else {
            editor
                .state
                .selection
                .iter()
                .filter(|entity| {
                    resolved
                        .get(*entity)
                        .is_some_and(|row| row.record.contains_key("ReflectionProbe"))
                })
                .cloned()
                .collect()
        };
        if entities.is_empty() {
            return Err("Select one or more Reflection Probes to recapture".into());
        }
        for entity in &entities {
            let value = resolved
                .get(entity)
                .and_then(|row| row.record.get("ReflectionProbe"))
                .ok_or("That entity no longer has a ReflectionProbe")?;
            let probe: ara_components::render::ReflectionProbe =
                serde::Deserialize::deserialize(value)?;
            if let Some(reason) = probe.invalid() {
                return Err(reason.into());
            }
        }
        let response=editor.action("render.probe.recapture",json!({"doc":editor.state.doc,"entities":entities,"why":"Recapture authored reflection probes"}))?;
        let id = response["job"]
            .as_str()
            .ok_or("Probe recapture returned no Host job receipt")?
            .to_owned();
        self.probe_ui.jobs.insert(
            id.clone(),
            Job {
                doc: editor.state.doc.clone(),
                entities,
                latest: None,
                error: None,
            },
        );
        self.probe_ui.job_order.push_back(id);
        self.probe_ui.next_poll = None;
        self.poll_probes(editor)?;
        self.rebuild(editor)
    }

    pub(crate) fn cancel_probe_capture(
        &mut self,
        editor: &mut Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        let ids: Vec<_> = match target {
            commands::Target::Text(id) => vec![id.clone()],
            _ => self
                .probe_ui
                .job_order
                .iter()
                .filter(|id| {
                    self.probe_ui.jobs.get(*id).is_some_and(|job| {
                        job.doc == editor.state.doc
                            && job.running()
                            && job.entities.iter().any(|entity| {
                                if let commands::Target::Entity(selected) = target {
                                    entity == selected
                                } else {
                                    editor.state.selection.contains(entity)
                                }
                            })
                    })
                })
                .cloned()
                .collect(),
        };
        if ids.is_empty() {
            return Err(
                "No active probe requests from this editor session match the selected probe".into(),
            );
        }
        let mut requested = 0;
        let mut failures = 0;
        for id in &ids {
            let job = self
                .probe_ui
                .jobs
                .get(id)
                .ok_or("That probe job is not owned by this editor UI session")?;
            if !job.running() {
                continue;
            }
            match editor.action("render.probe.cancel", json!({"job":id})) {
                Ok(response) => requested += usize::from(response["cancelRequested"] == true),
                Err(error) => {
                    failures += 1;
                    self.probe_ui
                        .jobs
                        .get_mut(id)
                        .expect("owned cancellation target remains cached")
                        .error = Some(error.to_string());
                }
            }
        }
        self.status_note = Some(format!(
            "Cancellation requested for {requested} of {} own capture requests; {failures} requests could not be read/cancelled. Host cancellation does not establish renderer retirement.",
            ids.len()
        ));
        self.probe_ui.next_poll = None;
        self.poll_probes(editor)?;
        self.rebuild(editor)
    }

    pub(crate) fn copy_probe_state(
        &mut self,
        editor: &Editor,
        target: &commands::Target,
    ) -> ara_ui::Result<()> {
        let entity = target
            .entity(&editor.state)
            .ok_or("Select a Reflection Probe with a renderer snapshot")?;
        let row = self
            .probe_ui
            .row(entity)
            .ok_or("The renderer has not reported that Reflection Probe yet")?;
        let jobs: Vec<_> = self
            .probe_ui
            .job_order
            .iter()
            .filter_map(|id| self.probe_ui.jobs.get(id).map(|job| (id, job)))
            .filter(|(_, job)| {
                job.doc == editor.state.doc
                    && job.entities.iter().any(|candidate| candidate == entity)
            })
            .map(|(id, job)| json!({"job":id,"status":job.latest}))
            .collect();
        self.pending_clipboard = Some(ara_json::to_canonical_string(
            &json!({"doc":editor.state.doc,"entity":entity,
            "queryVersion":self.probe_ui.version,"currentSnapshotAvailable":self.probe_ui.available,
            "capabilities":self.probe_ui.capabilities,"probe":row,"ownedJobs":jobs}),
        ));
        Ok(())
    }
}
