//! Scoped native-editor profiling controls. Player messages use existing notifications.
mod decode;
mod handle;
mod ingress;
mod model;
pub use handle::PlayProfilingHandle;
pub use model::*;

use ara_diag::Diagnostic;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

const LIMIT: usize = 32;
const HISTORY: usize = 64;
const MAX_TARGETS: usize = 128;
pub(crate) const RESULT: &str = "player.private.profileResult";
const CONTROL: &str = "player.private.profile";

enum Message {
    Subscribe(SyncSender<Update>),
    Refresh(SyncSender<Update>),
    Request {
        request: PlayProfileRequest,
        status: Box<PlayProfileStatus>,
        reply: SyncSender<Update>,
    },
}
enum Update {
    Targets(Vec<PlayProfileTargetRow>),
    Status(PlayProfileStatus),
}
struct Task {
    status: Option<PlayProfileStatus>,
    reply: SyncSender<Update>,
    started: Instant,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    revision: u64,
    ticket: u64,
    available: bool,
    recording: bool,
    reason: Option<String>,
    outcome: Option<PlayProfileOutcome>,
    profile: Option<ara_profile::Profile>,
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
}
pub(crate) struct Controller {
    tx: SyncSender<Message>,
    rx: Receiver<Message>,
    sequence: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    rows: Vec<PlayProfileTargetRow>,
    subscribers: Vec<SyncSender<Update>>,
    tasks: Vec<Task>,
    deferred: Option<Message>,
    dirty: bool,
    decoder: Option<decode::Decoder>,
    query_at: Instant,
    revisions: BTreeMap<PlayProfileTarget, u64>,
}
impl Default for Controller {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(LIMIT);
        Self {
            tx,
            rx,
            sequence: Arc::new(AtomicU64::new(1)),
            live: Arc::new(AtomicBool::new(true)),
            rows: Vec::new(),
            subscribers: Vec::new(),
            tasks: Vec::new(),
            deferred: None,
            dirty: false,
            decoder: None,
            query_at: Instant::now(),
            revisions: BTreeMap::new(),
        }
    }
}
fn problem(root: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(crate::codes::PLAY, message)
        .file(root.join("ara.project.json").to_string_lossy())
}
impl Controller {
    pub(crate) fn handle(&self, root: &Path) -> PlayProfilingHandle {
        handle::create(self, root)
    }

    pub(crate) fn tick(
        &mut self,
        worlds: &mut BTreeMap<String, crate::World>,
        sources: &mut crate::scripts::DiagnosticSources,
        root: &Path,
        publish: &mut impl FnMut(Diagnostic),
    ) {
        if self.subscribers.is_empty() && self.tasks.is_empty() && self.deferred.is_none() {
            let Ok(message) = self.rx.try_recv() else {
                return;
            };
            self.deferred = Some(message);
        }
        for row in &mut self.rows {
            let exists = worlds
                .get(&row.target.world)
                .filter(|world| world.fault.is_none())
                .and_then(|world| world.players.get(row.target.player as usize))
                .is_some_and(|(_, player)| player.incarnation == row.target.source);
            if !exists && row.reason.as_deref() != Some("The selected player incarnation exited") {
                row.available = false;
                row.recording = false;
                row.reason = Some("The selected player incarnation exited".into());
                self.dirty = true;
            }
        }
        for (world, state) in worlds.iter_mut().filter(|(_, state)| state.fault.is_none()) {
            for (index, (role, player)) in state.players.iter_mut().enumerate() {
                if !self.rows.iter().any(|row| {
                    row.target.world == *world
                        && row.target.player as usize == index
                        && row.target.source == player.incarnation
                }) {
                    let Ok(binding) = sources.binding(world, index, &player.incarnation) else {
                        continue;
                    };
                    if self.rows.len() >= MAX_TARGETS {
                        if let Some(index) = self.rows.iter().position(|row| {
                            row.reason.as_deref() == Some("The selected player incarnation exited")
                        }) {
                            let removed = self.rows.remove(index);
                            self.revisions.remove(&removed.target);
                        } else {
                            continue;
                        }
                    }
                    self.rows.push(PlayProfileTargetRow {
                        target: PlayProfileTarget {
                            world: world.clone(),
                            player: index as u32,
                            source: player.incarnation.clone(),
                            binding,
                        },
                        name: format!(
                            "{} / {} / {role:?} ({})",
                            world,
                            state.scene,
                            if player.hello.window {
                                "windowed"
                            } else {
                                "headless"
                            }
                        ),
                        process_id: player.hello.pid,
                        recording: false,
                        available: false,
                        reason: Some("Waiting for the player recorder".into()),
                    });
                    let _ = player
                        .link
                        .notify(CONTROL, json!({"ticket":0,"action":"query"}));
                    self.dirty = true;
                }
                for value in std::mem::take(&mut player.profiling) {
                    if value["profile"].is_null() {
                        self.receive(world, index, &player.incarnation, value, root, publish);
                    } else {
                        let input = decode::Incoming {
                            world: world.clone(),
                            player: index,
                            source: player.incarnation.clone(),
                            value,
                        };
                        if self.decoder.is_none() {
                            self.decoder = decode::Decoder::new().ok();
                        }
                        match &self.decoder {
                            Some(decoder) => {
                                if let Err(input) = decoder.submit(input) {
                                    self.decode_failed(
                                        &input.world,
                                        input.player,
                                        &input.source,
                                        input.value["ticket"].as_u64().unwrap_or(0),
                                        root,
                                        publish,
                                    );
                                }
                            }
                            None => self.decode_failed(
                                world,
                                index,
                                &player.incarnation,
                                input.value["ticket"].as_u64().unwrap_or(0),
                                root,
                                publish,
                            ),
                        }
                    }
                }
            }
        }
        for _ in 0..LIMIT {
            let Some(message) = self.deferred.take().or_else(|| self.rx.try_recv().ok()) else {
                break;
            };
            match message {
                Message::Subscribe(reply) => {
                    if self.subscribers.len() < LIMIT {
                        self.subscribers.push(reply);
                        self.dirty = true;
                    }
                }
                Message::Refresh(reply) => {
                    let _ = reply.try_send(Update::Targets(self.rows.clone()));
                    if Instant::now() >= self.query_at {
                        self.query_at = Instant::now() + Duration::from_millis(500);
                        for state in worlds.values_mut().filter(|state| state.fault.is_none()) {
                            for (_, player) in &mut state.players {
                                let _ = player
                                    .link
                                    .notify(CONTROL, json!({"ticket":0,"action":"query"}));
                            }
                        }
                    }
                }
                Message::Request {
                    request,
                    status,
                    reply,
                } => {
                    if self.tasks.len() >= LIMIT {
                        self.deferred = Some(Message::Request {
                            request,
                            status,
                            reply,
                        });
                        break;
                    }
                    let mut status = *status;
                    let row = self
                        .rows
                        .iter()
                        .find(|row| row.target == request.target && row.available);
                    if let Some(row) = row {
                        status.recording = row.recording;
                        let player = &mut worlds
                            .get_mut(&request.target.world)
                            .expect("available target world exists")
                            .players[request.target.player as usize]
                            .1;
                        if let Err(error) = player
                            .link
                            .notify(
                                CONTROL,
                                json!({"ticket":status.ticket.0,"action":request.action}),
                            )
                            .and_then(|_| player.link.flush())
                        {
                            finish(
                                &mut status,
                                PlayProfileOutcome::Failed,
                                problem(
                                    root,
                                    format!("Player profiling transport failed: {error}"),
                                ),
                                publish,
                            );
                        }
                    } else {
                        finish(
                            &mut status,
                            PlayProfileOutcome::Refused,
                            problem(
                                root,
                                "Profiling refused: selected world/player/source/binding is unavailable or stale",
                            ),
                            publish,
                        );
                    }
                    self.tasks.push(Task {
                        status: Some(status),
                        reply,
                        started: Instant::now(),
                    });
                }
            }
        }
        for _ in 0..4 {
            let Some(decoded) = self.decoder.as_ref().and_then(decode::Decoder::poll) else {
                break;
            };
            match decoded.result {
                Ok(reply) => self.accept(
                    &decoded.world,
                    decoded.player,
                    &decoded.source,
                    reply,
                    root,
                    publish,
                ),
                Err(_) => self.decode_failed(
                    &decoded.world,
                    decoded.player,
                    &decoded.source,
                    decoded.ticket,
                    root,
                    publish,
                ),
            }
        }
        for task in &mut self.tasks {
            let status = task
                .status
                .as_mut()
                .expect("task retains its status until publication");
            if status.finished {
                continue;
            }
            if !self
                .rows
                .iter()
                .any(|row| row.target == status.target && row.available)
            {
                finish(
                    status,
                    PlayProfileOutcome::Refused,
                    problem(
                        root,
                        "Profiling refused: selected incarnation exited before completion",
                    ),
                    publish,
                );
            } else if worlds
                .get(&status.target.world)
                .and_then(|world| world.players.get(status.target.player as usize))
                .is_some_and(|(_, player)| player.grace > Instant::now())
            {
                task.started = Instant::now();
            } else if task.started.elapsed() > Duration::from_secs(30) {
                finish(
                    status,
                    PlayProfileOutcome::Failed,
                    problem(root, "Player profiling control stalled for 30 seconds"),
                    publish,
                );
            }
        }
        if self.dirty {
            self.subscribers.retain(|subscriber| {
                !matches!(
                    subscriber.try_send(Update::Targets(self.rows.clone())),
                    Err(mpsc::TrySendError::Disconnected(_))
                )
            });
            self.dirty = false;
        }
        self.tasks.retain_mut(|task| {
            if !task.status.as_ref().is_some_and(|status| status.finished) {
                return true;
            }
            let status = task.status.take().expect("completed status exists");
            match task.reply.try_send(Update::Status(status)) {
                Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => false,
                Err(mpsc::TrySendError::Full(Update::Status(status))) => {
                    task.status = Some(status);
                    true
                }
                Err(mpsc::TrySendError::Full(Update::Targets(_))) => unreachable!("sent a status"),
            }
        });
    }
}
fn finish(
    status: &mut PlayProfileStatus,
    outcome: PlayProfileOutcome,
    diagnostic: Diagnostic,
    publish: &mut impl FnMut(Diagnostic),
) {
    publish(diagnostic.clone());
    status.diagnostics.push(diagnostic);
    status.outcome = Some(outcome);
    status.finished = true;
    status.profile = None;
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
