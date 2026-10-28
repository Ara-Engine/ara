//! Owned, bounded native-editor control of current Play script incarnations.
mod handle;
mod model;
pub use handle::ScriptRecoveryHandle;
pub use model::*;

use ara_diag::Diagnostic;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};

const LIMIT: usize = 32;
const HISTORY: usize = 64;
const METHOD: &str = "player.private.scriptRecovery";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    world: String,
    player: u32,
    source: String,
    binding: u64,
}

enum Message {
    Subscribe(SyncSender<Update>),
    Request {
        status: ScriptRecoveryStatus,
        targets: Vec<Target>,
        reply: SyncSender<Update>,
    },
}
enum Update {
    Roster(Vec<Target>),
    Status(ScriptRecoveryStatus),
}
struct Task {
    status: ScriptRecoveryStatus,
    waiting: Vec<(Target, u64)>,
    reply: SyncSender<Update>,
    dirty: bool,
}
struct Subscriber {
    reply: SyncSender<Update>,
    revision: u64,
}

pub(crate) struct Controller {
    tx: SyncSender<Message>,
    rx: Receiver<Message>,
    sequence: Arc<AtomicU64>,
    live: Arc<AtomicBool>,
    roster: Vec<Target>,
    revision: u64,
    subscribers: Vec<Subscriber>,
    tasks: Vec<Task>,
    deferred: Option<Message>,
}

impl Default for Controller {
    fn default() -> Self {
        let (tx, rx) = mpsc::sync_channel(LIMIT);
        Self {
            tx,
            rx,
            sequence: Arc::new(AtomicU64::new(1)),
            live: Arc::new(AtomicBool::new(true)),
            roster: Vec::new(),
            revision: 1,
            subscribers: Vec::new(),
            tasks: Vec::new(),
            deferred: None,
        }
    }
}

fn problem(root: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(crate::codes::PLAY, message)
        .file(root.join("ara.project.json").to_string_lossy())
}

impl Controller {
    pub(crate) fn handle(&self, root: &Path) -> ScriptRecoveryHandle {
        handle::create(
            self.tx.clone(),
            Arc::clone(&self.sequence),
            Arc::clone(&self.live),
            root.to_path_buf(),
        )
    }

    pub(crate) fn tick(
        &mut self,
        worlds: &mut BTreeMap<String, crate::World>,
        sources: &mut crate::scripts::DiagnosticSources,
        root: &Path,
        publish: &mut impl FnMut(Diagnostic),
    ) {
        // Compare borrowed identities first; allocate a roster only on an incarnation change.
        let unchanged = worlds
            .iter()
            .filter(|(_, world)| world.fault.is_none())
            .flat_map(|(world, state)| {
                state
                    .players
                    .iter()
                    .enumerate()
                    .map(move |(index, (_, player))| (world, index, &player.incarnation))
            })
            .eq(self
                .roster
                .iter()
                .map(|t| (&t.world, t.player as usize, &t.source)));
        if !unchanged {
            self.roster.clear();
            for (world, state) in worlds.iter().filter(|(_, state)| state.fault.is_none()) {
                for (index, (_, player)) in state.players.iter().enumerate() {
                    if let Ok(binding) = sources.binding(world, index, &player.incarnation) {
                        self.roster.push(Target {
                            world: world.clone(),
                            player: index as u32,
                            source: player.incarnation.clone(),
                            binding,
                        });
                    }
                }
            }
            self.revision = self.revision.wrapping_add(1);
        }
        for _ in 0..LIMIT {
            let Some(message) = self.deferred.take().or_else(|| self.rx.try_recv().ok()) else {
                break;
            };
            match message {
                Message::Subscribe(reply) => {
                    if self.subscribers.len() < LIMIT {
                        self.subscribers.push(Subscriber { reply, revision: 0 });
                    }
                }
                Message::Request {
                    mut status,
                    targets,
                    reply,
                } => {
                    if self.tasks.len() >= LIMIT {
                        // Preserve an accepted ticket even when its reply mailbox is full.
                        self.deferred = Some(Message::Request {
                            status,
                            targets,
                            reply,
                        });
                        break;
                    }
                    for diagnostic in &status.diagnostics {
                        publish(diagnostic.clone());
                    }
                    let mut waiting = Vec::new();
                    for (target, result) in targets.into_iter().zip(&mut status.players) {
                        if !self.roster.contains(&target) {
                            result.outcome = ScriptRecoveryOutcome::Refused;
                            let diagnostic = problem(
                                root,
                                "Script recovery refused: the selected Play incarnation has stopped or changed",
                            );
                            publish(diagnostic.clone());
                            result.diagnostics.push(diagnostic);
                            continue;
                        }
                        let existing = self
                            .tasks
                            .iter()
                            .flat_map(|task| &task.waiting)
                            .find(|(other, _)| other == &target)
                            .map(|(_, wire)| *wire);
                        let wire = existing.unwrap_or(status.ticket.0);
                        if existing.is_none() {
                            let player = &mut worlds
                                .get_mut(&target.world)
                                .expect("roster world exists")
                                .players[target.player as usize]
                                .1;
                            if let Err(error) = player
                                .link
                                .notify(METHOD, json!({"ticket":wire}))
                                .and_then(|_| player.link.flush())
                            {
                                result.outcome = ScriptRecoveryOutcome::Failed;
                                let diagnostic = problem(
                                    root,
                                    format!(
                                        "Script recovery could not reach the selected player: {error}"
                                    ),
                                );
                                publish(diagnostic.clone());
                                result.diagnostics.push(diagnostic);
                                continue;
                            }
                        }
                        waiting.push((target, wire));
                    }
                    status.finished = waiting.is_empty();
                    self.tasks.push(Task {
                        status,
                        waiting,
                        reply,
                        dirty: true,
                    });
                }
            }
        }
        for task in &mut self.tasks {
            task.waiting.retain(|(target, _)| {
                if self.roster.contains(target) { return true; }
                if let Some(result) = task.status.players.iter_mut().find(|r| r.player == target.player) {
                    result.outcome = ScriptRecoveryOutcome::Refused;
                    let diagnostic = problem(root, "Script recovery refused: the selected Play incarnation exited before completion");
                    publish(diagnostic.clone());
                    result.diagnostics.push(diagnostic);
                }
                task.dirty = true;
                false
            });
            task.status.finished = task.waiting.is_empty();
        }
        self.subscribers.retain_mut(|subscriber| {
            if subscriber.revision == self.revision {
                return true;
            }
            match subscriber
                .reply
                .try_send(Update::Roster(self.roster.clone()))
            {
                Ok(()) => {
                    subscriber.revision = self.revision;
                    true
                }
                Err(mpsc::TrySendError::Full(_)) => true,
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            }
        });
        self.tasks.retain_mut(|task| {
            if !task.dirty {
                return true;
            }
            match task.reply.try_send(Update::Status(task.status.clone())) {
                Ok(()) => {
                    task.dirty = false;
                    !task.status.finished
                }
                Err(mpsc::TrySendError::Full(_)) => true,
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            }
        });
    }

    pub(crate) fn observe(
        &mut self,
        world: &str,
        player: usize,
        source: &str,
        diagnostic: &Diagnostic,
    ) {
        let recovery = diagnostic
            .data
            .as_ref()
            .and_then(|data| data.get("recovery"));
        let terminal = recovery.filter(|result| {
            diagnostic.code == ara_script::codes::RECOVERY_RESULT
                && result["schema"] == 1
                && result["ticket"].as_u64().is_some_and(|ticket| ticket > 0)
        });
        let ticket = terminal.and_then(|result| result["ticket"].as_u64());
        for task in &mut self.tasks {
            let Some(index) = task.waiting.iter().position(|(target, wire)| {
                target.world == world
                    && target.player as usize == player
                    && target.source == source
                    && ticket.is_none_or(|ticket| ticket == *wire)
            }) else {
                continue;
            };
            let target = &task.waiting[index].0;
            let result = task
                .status
                .players
                .iter_mut()
                .find(|result| result.player == target.player)
                .expect("pending player has a result");
            if let Some(terminal) = terminal {
                result.outcome = match terminal["outcome"].as_str() {
                    Some("reloaded") => ScriptRecoveryOutcome::Reloaded,
                    Some("unchanged") => ScriptRecoveryOutcome::Unchanged,
                    Some("refused") => ScriptRecoveryOutcome::Refused,
                    _ => ScriptRecoveryOutcome::Failed,
                };
                result.modules = terminal["modules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|module| module.as_str().map(str::to_owned))
                    .collect();
                task.waiting.remove(index);
            }
            if result.diagnostics.len() < 128 || terminal.is_some() {
                result.diagnostics.push(diagnostic.clone());
            }
            task.dirty = true;
            task.status.finished = task.waiting.is_empty();
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
