//! Nonblocking runtime reads, guarded edits and one-transaction Keep.
use super::*;
use std::time::{Duration, Instant};

enum Phase {
    Inspect {
        player: usize,
        entity: String,
    },
    Fresh,
    Patched,
    Refreshed,
    Keep,
    Control {
        player: usize,
        spectator: bool,
        resume: bool,
    },
    View {
        player: usize,
        spectator: bool,
        resume: bool,
    },
}
pub(super) struct Pending {
    id: String,
    world: String,
    phase: Phase,
    polled: Instant,
}

impl Shell {
    fn play_idle(&self) -> ara_ui::Result<()> {
        if self.play_controls.pending.is_some() {
            return Err("Wait for the current runtime operation to complete".into());
        }
        Ok(())
    }
    fn play_job(
        &mut self,
        editor: &mut Editor,
        method: &str,
        mut params: Value,
        phase: Phase,
    ) -> ara_ui::Result<()> {
        let world = editor
            .play_world
            .as_ref()
            .ok_or("Start Play first")?
            .clone();
        params["world"] = json!(world);
        if !matches!(method, "play.view" | "play.control") {
            params["background"] = json!(true);
        }
        let reply = editor.action(method, params)?;
        let id = reply["job"]
            .as_str()
            .ok_or("The Play operation did not return a job")?
            .to_owned();
        self.play_controls.pending = Some(Pending {
            id,
            world,
            phase,
            polled: Instant::now(),
        });
        self.play_controls.message = Some("Waiting for the runtime boundary…".into());
        Ok(())
    }
    pub(crate) fn inspect_play_selection(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.play_idle()?;
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty())
        {
            return Err("Apply or discard pending runtime edits before refreshing".into());
        }
        let entity = editor
            .state
            .selection
            .first()
            .ok_or("Select an entity in Hierarchy first")?
            .clone();
        let player = self.game_client;
        self.play_job(
            editor,
            "play.inspect",
            json!({"player":player,"entity":entity}),
            Phase::Inspect { player, entity },
        )?;
        self.play_controls.options = true;
        self.rebuild(editor)
    }
    pub(crate) fn commit_play_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.play_controls.pending.is_some() {
            return Ok(false);
        }
        let Some(runtime) = self
            .play_controls
            .runtime
            .as_ref()
            .filter(|runtime| !runtime.drafts.is_empty())
        else {
            return Ok(false);
        };
        if !runtime.editable {
            return Err("Only the authority's runtime values can be edited".into());
        }
        validate_target(editor, self, runtime)?;
        self.play_job(
            editor,
            "play.inspect",
            json!({"player":runtime.player,"entity":runtime.entity}),
            Phase::Fresh,
        )?;
        Ok(true)
    }
    pub(crate) fn keep_play_changes(
        &mut self,
        editor: &mut Editor,
        include_driven: bool,
    ) -> ara_ui::Result<()> {
        self.play_idle()?;
        if editor.state.selection.is_empty() {
            return Err("Select the entities whose changes should be kept".into());
        }
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty())
        {
            return Err("Apply or discard runtime drafts first".into());
        }
        self.play_job(editor,"play.keep",json!({"entities":editor.state.selection,"includeDriven":include_driven,"why":"Keep selected runtime changes"}),Phase::Keep)?;
        self.rebuild(editor)
    }
    /// Poll at a modest cadence; these Host job lookups never wait on player IO.
    pub(crate) fn poll_play_job(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        if self.play_controls.pending.is_none()
            && self.game_running
            && let Some(resume) = self.play_controls.simulate.take()
        {
            self.control_play(editor, None, resume)?;
            return Ok(true);
        }
        let Some(mut pending) = self.play_controls.pending.take() else {
            return Ok(false);
        };
        if editor.play_world.as_deref() != Some(pending.world.as_str()) {
            let _ = editor.action("jobs.cancel", json!({"job":pending.id}));
            self.play_controls.runtime = None;
            self.play_controls.spectators.clear();
            self.play_controls.simulate = None;
            self.play_controls.message =
                Some("The runtime operation ended because Play stopped or changed".into());
            return Ok(true);
        }
        if pending.polled.elapsed() < Duration::from_millis(50) {
            self.play_controls.pending = Some(pending);
            return Ok(false);
        }
        pending.polled = Instant::now();
        let status = editor.action("jobs.status", json!({"job":pending.id}))?;
        match status["status"].as_str() {
            Some("running") => {
                self.play_controls.pending = Some(pending);
                Ok(false)
            }
            Some("done") => {
                let result = self.complete_play_job(editor, pending, &status["result"]);
                if let Err(error) = &result {
                    self.play_controls.message = Some(error.to_string());
                }
                result.map(|()| true)
            }
            _ => {
                let message = status["diagnostics"][0]["message"].as_str().unwrap_or(
                    "The runtime operation was cancelled or failed; scene values were preserved",
                );
                self.play_controls.message = Some(message.into());
                if let Some(diags) = status["diagnostics"].as_array() {
                    editor.action("diag.report", json!({"diagnostics":diags}))?;
                }
                Ok(true)
            }
        }
    }
    fn complete_play_job(
        &mut self,
        editor: &mut Editor,
        pending: Pending,
        value: &Value,
    ) -> ara_ui::Result<()> {
        match pending.phase {
            Phase::Inspect { player, entity } => {
                if self.game_client != player || editor.state.selection.first() != Some(&entity) {
                    return Err("The selection changed while inspecting; inspect again".into());
                }
                let components = value
                    .get("components")
                    .filter(|value| value.is_object())
                    .ok_or("Inspection returned no component values")?
                    .clone();
                let status = editor.action("play.status", json!({}))?;
                let editable = player == 0
                    && status["worlds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|row| row["world"] == pending.world)
                        .and_then(|row| row["players"].as_array())
                        .and_then(|players| players.first())
                        .is_some_and(|player| {
                            matches!(
                                player["role"].as_str(),
                                Some("local" | "host" | "dedicated")
                            )
                        });
                self.play_controls.runtime = Some(Runtime {
                    world: pending.world,
                    player,
                    entity,
                    tick: value["tick"].as_u64().unwrap_or(0),
                    components,
                    editable,
                    drafts: BTreeMap::new(),
                });
                self.play_controls.message = None;
            }
            Phase::Fresh => {
                let runtime = self
                    .play_controls
                    .runtime
                    .as_ref()
                    .ok_or("The runtime inspection was cleared")?;
                validate_target(editor, self, runtime)?;
                let sets = runtime::sets(runtime, value)?;
                self.play_job(editor, "play.patch", json!({"sets":sets}), Phase::Patched)?;
            }
            Phase::Patched => {
                let runtime = self
                    .play_controls
                    .runtime
                    .as_mut()
                    .ok_or("The runtime inspection was cleared")?;
                runtime.drafts.clear();
                let player = runtime.player;
                let entity = runtime.entity.clone();
                self.play_job(
                    editor,
                    "play.inspect",
                    json!({"player":player,"entity":entity}),
                    Phase::Refreshed,
                )?;
            }
            Phase::Refreshed => {
                let runtime = self
                    .play_controls
                    .runtime
                    .as_mut()
                    .ok_or("The runtime inspection was cleared")?;
                runtime.components = value["components"].clone();
                runtime.tick = value["tick"].as_u64().unwrap_or(runtime.tick);
                self.play_controls.message = Some(format!(
                    "Runtime edits applied at tick {}. Keep selected changes to save them to the scene.",
                    runtime.tick
                ));
            }
            Phase::Keep => {
                let count = |key: &str| value[key].as_array().map_or(0, Vec::len);
                self.play_controls.message = Some(format!(
                    "Kept {} fields · {} conflicts preserved · {} unsupported changes skipped. One Undo restores the kept scene values.",
                    count("kept"),
                    count("conflicts"),
                    count("skipped")
                ));
                if count("conflicts") > 0 {
                    self.dialog = Some(Dialog::Info {
                        title: "Runtime changes conflict with scene edits".into(),
                        lines: vec!["Conflicting scene values were preserved.".into()],
                        code: Some(ara_json::to_canonical_string(&value["conflicts"])),
                    });
                }
            }
            Phase::Control {
                player,
                spectator,
                resume,
            } => {
                self.play_job(
                    editor,
                    "play.view",
                    json!({"player":player,"camera":null,"captureCurrent":spectator,"navigationSpeed":editor.state.number("viewport.cameraSpeed",4.)}),
                    Phase::View {
                        player,
                        spectator,
                        resume,
                    },
                )?;
            }
            Phase::View {
                player,
                spectator,
                resume,
            } => {
                if spectator {
                    self.play_controls.spectators.insert(player);
                } else {
                    self.play_controls.spectators.remove(&player);
                }
                if resume {
                    editor.action("play.resume", json!({"world":pending.world}))?;
                    editor.action("editor.view", json!({"play":"playing"}))?;
                }
                self.game_captured = false;
                self.play_controls.message=Some(if spectator {"Spectator camera active. Click this player's view to navigate; simulation continues."}else{"Selected actor possessed. Click the view to control it."}.into());
            }
        }
        Ok(())
    }
    /// Eject/possess through the authority's typed op boundary, then change only this player's view.
    pub(crate) fn control_play(
        &mut self,
        editor: &mut Editor,
        entity: Option<String>,
        resume: bool,
    ) -> ara_ui::Result<()> {
        self.play_idle()?;
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty())
        {
            return Err("Apply or discard runtime drafts before changing control".into());
        }
        let player = self.game_client;
        let spectator = entity.is_none();
        if let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces) {
            surfaces.device(
                world,
                player,
                vec![ara_play_host::embed::DeviceEvent::FocusLost],
            );
        }
        self.game_captured = false;
        self.play_job(
            editor,
            "play.control",
            json!({"player":player,"entity":entity}),
            Phase::Control {
                player,
                spectator,
                resume,
            },
        )?;
        self.rebuild(editor)
    }
}
fn validate_target(editor: &Editor, shell: &Shell, runtime: &Runtime) -> ara_ui::Result<()> {
    if editor.play_world.as_deref() != Some(runtime.world.as_str())
        || shell.game_client != runtime.player
        || editor.state.selection.first().map(String::as_str) != Some(runtime.entity.as_str())
    {
        return Err("The Play world, client or selection changed; inspect again".into());
    }
    Ok(())
}
