//! One recorder per single-use player/world, with worker-owned capture serialization.
use ara_play::link::{Notifications, Pump};
use ara_play_host::{PlayProfileAction, PlayProfileOutcome};
use ara_profile::{Config, Counters, Memory, Profile, Profiler, ThreadRegistration};
use serde::Serialize;
use serde_json::Value;
use std::sync::mpsc::{self, SyncSender};

pub(super) const CONTROL: &str = "player.private.profile";
const RESULT: &str = "player.private.profileResult";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    revision: u64,
    ticket: u64,
    available: bool,
    recording: bool,
    reason: Option<String>,
    outcome: Option<PlayProfileOutcome>,
    profile: Option<Profile>,
    diagnostics: Vec<ara_diag::Diagnostic>,
}

#[derive(Default)]
pub(super) struct Recorder {
    profiler: Option<Profiler>,
    open: bool,
    reason: Option<String>,
    exports: Option<SyncSender<Reply>>,
    revision: u64,
}

impl Recorder {
    fn ensure(&mut self) -> Result<&mut Profiler, String> {
        if self.profiler.is_none() {
            let mut profiler = Profiler::new(Config::default()).map_err(str::to_owned)?;
            profiler
                .register_thread("player.main")
                .attach()
                .map_err(str::to_owned)?;
            self.profiler = Some(profiler);
        }
        self.profiler
            .as_mut()
            .ok_or_else(|| "Player recorder is unavailable".into())
    }
    fn recording(&self) -> bool {
        self.profiler.as_ref().is_some_and(Profiler::is_recording)
    }

    /// A control notification runs between frames; captures cannot include an open frame.
    pub(super) fn control(
        &mut self,
        link: &mut Pump,
        params: Value,
        has_world: bool,
    ) -> anyhow::Result<()> {
        let ticket = params["ticket"].as_u64().unwrap_or(0);
        self.revision = self.revision.saturating_add(1);
        if params["action"] == "query" {
            let reason = if has_world {
                self.reason.clone()
            } else {
                Some("The selected player has no running world".into())
            };
            link.notify(
                RESULT,
                Reply {
                    revision: self.revision,
                    ticket: 0,
                    available: reason.is_none(),
                    recording: self.recording(),
                    reason,
                    outcome: None,
                    profile: None,
                    diagnostics: Vec::new(),
                },
            )?;
            return Ok(());
        }
        let action = serde_json::from_value::<PlayProfileAction>(params["action"].clone());
        let frame_open = self.open;
        let mut reply = Reply {
            revision: self.revision,
            ticket,
            available: has_world && self.reason.is_none(),
            recording: self.recording(),
            reason: self.reason.clone(),
            outcome: Some(PlayProfileOutcome::Refused),
            profile: None,
            diagnostics: Vec::new(),
        };
        if !has_world {
            reply.reason = Some("The selected player has no running world".into());
        } else if let Ok(action) = action {
            if action == PlayProfileAction::Stop && !self.recording() {
                reply.outcome = Some(PlayProfileOutcome::Unchanged);
                link.notify(RESULT, reply)?;
                return Ok(());
            }
            match self.ensure() {
                Ok(profiler) => {
                    let was_recording = profiler.is_recording();
                    match action {
                        PlayProfileAction::Start => profiler.start(),
                        PlayProfileAction::Stop => profiler.stop(),
                        PlayProfileAction::Snapshot => {}
                        PlayProfileAction::Freeze => profiler.stop(),
                    }
                    profiler.collect();
                    reply.recording = profiler.is_recording();
                    reply.outcome = Some(
                        if (action == PlayProfileAction::Start && was_recording)
                            || (action == PlayProfileAction::Stop && !was_recording)
                        {
                            PlayProfileOutcome::Unchanged
                        } else {
                            PlayProfileOutcome::Completed
                        },
                    );
                    if matches!(
                        action,
                        PlayProfileAction::Snapshot | PlayProfileAction::Freeze
                    ) {
                        reply.profile = Some(completed_profile(profiler, frame_open));
                    }
                }
                Err(error) => {
                    self.reason = Some(error.clone());
                    reply.reason = Some(error);
                    reply.available = false;
                    reply.outcome = Some(PlayProfileOutcome::Failed);
                }
            }
        } else {
            reply.reason = Some("Unknown private profiling action".into());
        }
        if let Some(reason) = &reply.reason {
            reply.diagnostics.push(ara_diag::Diagnostic::error(
                ara_play_host::codes::PLAY,
                reason,
            ));
        }
        if reply.profile.is_none() {
            link.notify(RESULT, reply)?;
            return Ok(());
        }
        // The expensive JSON encoding never runs on the simulation or heartbeat thread.
        if self.exports.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Reply>(2);
            let notifications = link.notifications();
            if let Err(error) = std::thread::Builder::new()
                .name("ara-player-profile-export".into())
                .spawn(move || export(rx, notifications))
            {
                reply.profile = None;
                reply.outcome = Some(PlayProfileOutcome::Failed);
                reply.diagnostics.push(ara_diag::Diagnostic::error(
                    ara_play_host::codes::PLAY,
                    format!("Cannot start player profile export worker: {error}"),
                ));
                link.notify(RESULT, reply)?;
                return Ok(());
            }
            self.exports = Some(tx);
        }
        if let Some(sender) = &self.exports
            && let Err(error) = sender.try_send(reply)
        {
            let mut reply = match error {
                mpsc::TrySendError::Full(reply) | mpsc::TrySendError::Disconnected(reply) => reply,
            };
            reply.profile = None;
            reply.outcome = Some(PlayProfileOutcome::Failed);
            reply.diagnostics.push(ara_diag::Diagnostic::error(
                ara_play_host::codes::PLAY,
                "Player profile export queue is full or disconnected",
            ));
            link.notify(RESULT, reply)?;
        }
        Ok(())
    }

    pub(super) fn begin(&mut self) -> Option<u64> {
        let profiler = self
            .profiler
            .as_mut()
            .filter(|profiler| profiler.is_recording())?;
        match profiler.begin_frame() {
            Ok(token) => {
                self.open = true;
                Some(token.id())
            }
            Err(error) => {
                profiler.stop();
                self.reason = Some(error.into());
                None
            }
        }
    }
    pub(super) fn end(&mut self, entities: u64) {
        if !std::mem::take(&mut self.open) {
            return;
        }
        if let Some(profiler) = &mut self.profiler
            && let Err(error) = profiler.end_frame(
                Counters {
                    entities,
                    ..Counters::default()
                },
                Memory::default(),
            )
        {
            profiler.stop();
            self.reason = Some(error.into());
        }
    }
    pub(super) fn frame_open(&self) -> bool {
        self.open
    }
    pub(super) fn gpu(&mut self, frame: u64, passes: &[ara_gpu::commands::PassTiming]) {
        if let Some(profiler) = &mut self.profiler {
            profiler.gpu_timings(frame, passes);
        }
    }
}
fn completed_profile(profiler: &Profiler, open: bool) -> Profile {
    let mut profile = profiler.profile().clone();
    if open {
        profile.frames.pop_back();
    }
    profile
}
fn export(receiver: mpsc::Receiver<Reply>, notifications: Notifications) {
    while let Ok(reply) = receiver.recv() {
        if notifications.notify(RESULT, reply).is_err() {
            break;
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        if self
            .profiler
            .as_ref()
            .is_some_and(Profiler::current_thread_attached)
        {
            ThreadRegistration::detach();
        }
    }
}

#[cfg(test)]
mod tests;
