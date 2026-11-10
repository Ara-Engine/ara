//! Play's runtime controls for the editor window: read the running world (Inspect), edit
//! its values (drafts applied as typed `play.patch` sets), save chosen runtime fields to the
//! scene in one undo step (Keep), take a player's actor away or give it the selection
//! (Eject, Possess), and start Play as Simulate (a spectator camera, no possessed actor).
//!
//! Every request is a Host job polled without waiting on player IO ([`PlayControls::poll`],
//! called each pass); a job from a world that has since stopped is cancelled, never
//! adopted. Authored values change only through Keep's typed batch.
mod keep;
mod runtime;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use ara_schema::ValueType;
use serde_json::{Value, json};

use crate::Editor;
pub(crate) use keep::Choice;

/// Per-client network conditions a Play start can shape (`play.networkProfiles`).
pub(crate) const PROFILES: &[(&str, &str)] = &[
    ("clean", "Clean · 0 ms"),
    ("lan", "LAN · 5 ms"),
    ("internet", "Internet · 50 ms"),
    ("mobile", "Mobile · 100 ms"),
    ("stress", "Stress · 200 ms, 2% loss"),
];

/// A runtime field an edit targets: its `Component.field` path, a JSON pointer inside it
/// (empty for the whole value), and its schema type and unit (to parse typed text).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldTarget {
    pub path: String,
    pub pointer: String,
    pub ty: ValueType,
    pub unit: Option<String>,
}

impl FieldTarget {
    fn key(&self) -> String {
        format!("{}#{}", self.path, self.pointer)
    }
}

/// An edit to a running value, not yet applied.
struct Draft {
    target: FieldTarget,
    input: Value,
}

/// The entity last read from the running world.
pub(crate) struct Inspection {
    world: String,
    /// The player it was read from.
    pub player: usize,
    /// The entity read.
    pub entity: String,
    /// The simulation tick it was read at.
    pub tick: u64,
    /// Its components' runtime values, by component then field.
    pub components: Value,
    /// Whether its values can be edited (only the authority's can).
    pub editable: bool,
    drafts: BTreeMap<String, Draft>,
}

impl Inspection {
    /// The draft typed for field `path` (pointer `pointer`), if any.
    pub fn draft(&self, path: &str, pointer: &str) -> Option<&Value> {
        self.drafts
            .get(&format!("{path}#{pointer}"))
            .map(|d| &d.input)
    }

    /// How many edits wait to be applied.
    pub fn drafts(&self) -> usize {
        self.drafts.len()
    }
}

/// What a running job does with its reply.
enum Phase {
    Inspect {
        player: usize,
        entity: String,
    },
    Fresh,
    Patched,
    Refreshed,
    Keep,
    KeepPreview {
        entities: Vec<String>,
        include_driven: bool,
    },
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

struct Pending {
    id: String,
    world: String,
    phase: Phase,
    polled: Instant,
}

/// Simulate asked for: once the world's player is ready, eject to a spectator camera,
/// then resume unless Play was to start paused.
struct Simulate {
    world: String,
    resume: bool,
}

/// Play's runtime controls for one window. Transient: it starts over with each world.
#[derive(Default)]
pub(crate) struct PlayControls {
    world: Option<String>,
    /// The player commands act on (0: the host or the only player).
    pub client: usize,
    inspection: Option<Inspection>,
    pending: Option<Pending>,
    simulate: Option<Simulate>,
    spectators: BTreeSet<usize>,
    keep: Option<Choice>,
    /// The last Keep's result (`kept`, `conflicts`, `skipped`).
    pub last_keep: Option<Value>,
    /// What the last request did or why it could not, for the window to show.
    pub message: Option<String>,
    /// The game should give the keyboard and mouse back (control changed hands).
    pub release: bool,
}

impl PlayControls {
    /// The entity last read from the running world.
    pub fn inspection(&self) -> Option<&Inspection> {
        self.inspection.as_ref()
    }

    /// The runtime fields offered to Keep, while choosing.
    pub fn keep(&self) -> Option<&Choice> {
        self.keep.as_ref()
    }

    /// Whether a runtime request is still running.
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.simulate.is_some()
    }

    /// Whether the controlled player looks through a spectator camera (after Eject).
    pub fn ejected(&self) -> bool {
        self.spectators.contains(&self.client)
    }

    /// Starts over when Play stopped or another world started: drafts, Keep choices and
    /// spectators belong to one world, and its running job is cancelled.
    fn reconcile(&mut self, editor: &mut Editor) -> bool {
        if self.world == editor.play_world {
            return false;
        }
        let simulate = self
            .simulate
            .take()
            .filter(|request| editor.play_world.as_deref() == Some(request.world.as_str()));
        if let Some(pending) = self.pending.take() {
            let _ = editor.action("jobs.cancel", json!({"job": pending.id}));
        }
        *self = Self {
            world: editor.play_world.clone(),
            simulate,
            ..Self::default()
        };
        true
    }

    fn idle(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.reconcile(editor);
        match self.pending {
            Some(_) => Err("Wait for the current runtime operation to complete".into()),
            None => Ok(()),
        }
    }

    fn no_drafts(&self, why: &str) -> ara_ui::Result<()> {
        match self
            .inspection
            .as_ref()
            .is_some_and(|i| !i.drafts.is_empty())
        {
            true => Err(why.into()),
            false => Ok(()),
        }
    }

    /// Starts `method` on the running world as a job whose reply `phase` handles.
    fn job(
        &mut self,
        editor: &mut Editor,
        method: &str,
        mut params: Value,
        phase: Phase,
    ) -> ara_ui::Result<()> {
        let world = editor.play_world.clone().ok_or("Start Play first")?;
        params["world"] = json!(world);
        if !matches!(method, "play.view" | "play.control") {
            params["background"] = json!(true);
        }
        let reply = editor.action(method, params)?;
        let id = reply["job"]
            .as_str()
            .ok_or("The Play operation did not return a job")?
            .to_owned();
        self.pending = Some(Pending {
            id,
            world,
            phase,
            polled: Instant::now(),
        });
        self.message = Some("Waiting for the running game…".into());
        Ok(())
    }

    /// Reads the selected entity from the controlled player.
    pub fn inspect(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        self.no_drafts("Apply or discard the runtime edits before reading again")?;
        let entity = editor
            .state
            .selection
            .first()
            .ok_or("Select an entity first")?
            .clone();
        let player = self.client;
        self.job(
            editor,
            "play.inspect",
            json!({"player": player, "entity": entity}),
            Phase::Inspect { player, entity },
        )
    }

    /// Types `input` (JSON, or text parsed by the field's type) into a running field;
    /// [`Self::apply`] sends the drafts.
    pub fn draft(&mut self, target: FieldTarget, input: Value) -> ara_ui::Result<()> {
        let inspection = self
            .inspection
            .as_mut()
            .ok_or("Read the running entity before editing it")?;
        inspection
            .drafts
            .insert(target.key(), Draft { target, input });
        Ok(())
    }

    /// Applies the drafts to the running world: reads it fresh, refuses a collection that
    /// changed under a positional edit, patches, and reads the result back.
    pub fn apply(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        let inspection = self
            .inspection
            .as_ref()
            .filter(|i| !i.drafts.is_empty())
            .ok_or("Edit a running value first")?;
        if !inspection.editable {
            return Err("Only the authority's running values can be edited".into());
        }
        self.check(editor)?;
        let (player, entity) = (inspection.player, inspection.entity.clone());
        self.job(
            editor,
            "play.inspect",
            json!({"player": player, "entity": entity}),
            Phase::Fresh,
        )
    }

    /// Drops the drafts.
    pub fn discard(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        if let Some(inspection) = &mut self.inspection {
            inspection.drafts.clear();
        }
        self.message = None;
        Ok(())
    }

    /// The inspection still matches the world, player and selection it was read for.
    fn check(&self, editor: &Editor) -> ara_ui::Result<()> {
        let Some(i) = &self.inspection else {
            return Err("The running entity was not read".into());
        };
        if editor.play_world.as_deref() != Some(i.world.as_str())
            || self.client != i.player
            || editor.state.selection.first() != Some(&i.entity)
        {
            return Err("The Play world, player or selection changed; read it again".into());
        }
        Ok(())
    }

    /// Lists the selection's runtime changes to choose from (Keep Runtime Changes);
    /// `include_driven` offers simulation-driven values (transforms moved by physics) too.
    pub fn keep_changes(
        &mut self,
        editor: &mut Editor,
        include_driven: bool,
    ) -> ara_ui::Result<()> {
        self.idle(editor)?;
        if editor.state.selection.is_empty() {
            return Err("Select the entities whose runtime changes to keep".into());
        }
        self.no_drafts("Apply or discard the runtime edits first")?;
        let entities = editor.state.selection.clone();
        self.job(
            editor,
            "play.diff",
            json!({"entities": entities}),
            Phase::KeepPreview {
                entities,
                include_driven,
            },
        )
    }

    /// Ticks or unticks one runtime field to keep.
    pub fn keep_field(&mut self, entity: &str, path: &str, on: bool) -> ara_ui::Result<()> {
        self.keep
            .as_mut()
            .ok_or("List the runtime changes first")?
            .pick(entity, path, on)
    }

    /// Offers simulation-driven fields to Keep, or stops offering them.
    pub fn keep_driven(&mut self, on: bool) -> ara_ui::Result<()> {
        self.keep
            .as_mut()
            .ok_or("List the runtime changes first")?
            .driven(on);
        Ok(())
    }

    /// Saves the ticked fields to the scene in one undo step. The host diffs again and
    /// rechecks authored conflicts as it commits.
    pub fn keep_apply(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        let choice = self
            .keep
            .as_ref()
            .ok_or("Choose runtime fields before keeping them")?;
        if editor.play_world.as_deref() != Some(choice.world()) {
            return Err("The Play world changed; list its runtime changes again".into());
        }
        let (fields, include_driven) = choice.request()?;
        self.job(
            editor,
            "play.keep",
            json!({"fields": fields, "includeDriven": include_driven, "why": "Keep runtime changes"}),
            Phase::Keep,
        )
    }

    /// Closes the Keep chooser.
    pub fn keep_cancel(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        self.keep = None;
        Ok(())
    }

    /// Lists the same entities' runtime changes again.
    pub fn keep_refresh(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.idle(editor)?;
        let choice = self.keep.as_ref().ok_or("List the runtime changes first")?;
        if editor.play_world.as_deref() != Some(choice.world()) {
            return Err("The Play world changed; choose its entities and Keep again".into());
        }
        let (entities, include_driven) = (choice.entities().to_vec(), choice.include_driven());
        self.job(
            editor,
            "play.diff",
            json!({"entities": entities}),
            Phase::KeepPreview {
                entities,
                include_driven,
            },
        )
    }

    /// Gives the controlled player `entity` to drive (Possess), or with none releases its
    /// actor for a spectator camera while the simulation runs on (Eject); `resume` then
    /// resumes a paused world.
    pub fn control(
        &mut self,
        editor: &mut Editor,
        entity: Option<String>,
        resume: bool,
    ) -> ara_ui::Result<()> {
        self.idle(editor)?;
        self.no_drafts("Apply or discard the runtime edits before changing control")?;
        let player = self.client;
        if let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces) {
            surfaces.device(
                world,
                player,
                vec![ara_play_host::embed::DeviceEvent::FocusLost],
            );
        }
        self.release = true;
        let spectator = entity.is_none();
        self.job(
            editor,
            "play.control",
            json!({"player": player, "entity": entity}),
            Phase::Control {
                player,
                spectator,
                resume,
            },
        )
    }

    /// Starts Play as Simulate: paused in the Game view, then (once its player is ready)
    /// ejected to a spectator camera and resumed unless Play starts paused. While Play
    /// runs, Simulate is Eject.
    pub fn simulate(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if editor.state.playing.is_some() {
            return self.control(editor, None, false);
        }
        let mut params = crate::shell::commands::table::play_params(editor);
        let resume = params["startPaused"] != true;
        params["startPaused"] = json!(true);
        params["scene"] = json!(editor.state.doc);
        params["background"] = json!(true);
        editor.embed_play(&mut params);
        if !params["embed"].is_object() {
            return Err("Simulate runs in the Game view: turn Play in the Game view on".into());
        }
        editor.action("editor.layout.show", json!({"panel": "game"}))?;
        editor.action("play.start", params)?;
        self.reconcile(editor);
        let world = editor
            .play_world
            .clone()
            .ok_or("Simulate needs a starting Play world")?;
        self.simulate = Some(Simulate { world, resume });
        editor.action("editor.view", json!({"play": "paused"}))?;
        Ok(())
    }

    /// Advances the running job (at most every 50 ms) and a queued Simulate once the player
    /// is `ready`; whether anything changed the window should show.
    pub fn poll(&mut self, editor: &mut Editor, ready: bool) -> bool {
        match self.step(editor, ready) {
            Ok(changed) => changed,
            Err(error) => {
                self.message = Some(error.to_string());
                true
            }
        }
    }

    fn step(&mut self, editor: &mut Editor, ready: bool) -> ara_ui::Result<bool> {
        let changed = self.reconcile(editor);
        if self.pending.is_none()
            && ready
            && let Some(request) = self.simulate.take()
        {
            self.control(editor, None, request.resume)?;
            return Ok(true);
        }
        let Some(mut pending) = self.pending.take() else {
            return Ok(changed);
        };
        if pending.polled.elapsed() < Duration::from_millis(50) {
            self.pending = Some(pending);
            return Ok(changed);
        }
        pending.polled = Instant::now();
        let status = editor.action("jobs.status", json!({"job": pending.id}))?;
        match status["status"].as_str() {
            Some("running") => {
                self.pending = Some(pending);
                Ok(changed)
            }
            Some("done") => self
                .complete(editor, pending, &status["result"])
                .map(|()| true),
            _ => {
                self.message = Some(
                    status["diagnostics"][0]["message"]
                        .as_str()
                        .unwrap_or("The runtime operation was cancelled or failed; scene values were preserved")
                        .into(),
                );
                if let Some(diagnostics) = status["diagnostics"].as_array() {
                    editor.action("diag.report", json!({"diagnostics": diagnostics}))?;
                }
                Ok(true)
            }
        }
    }

    fn complete(
        &mut self,
        editor: &mut Editor,
        pending: Pending,
        value: &Value,
    ) -> ara_ui::Result<()> {
        self.message = None;
        match pending.phase {
            Phase::KeepPreview {
                entities,
                include_driven,
            } => {
                self.keep = Some(Choice::read(
                    pending.world,
                    entities,
                    include_driven,
                    value,
                )?);
            }
            Phase::Inspect { player, entity } => {
                if self.client != player || editor.state.selection.first() != Some(&entity) {
                    return Err("The selection changed while reading; read again".into());
                }
                let components = value
                    .get("components")
                    .filter(|v| v.is_object())
                    .ok_or("The running game returned no component values")?
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
                        .is_some_and(|p| {
                            matches!(p["role"].as_str(), Some("local" | "host" | "dedicated"))
                        });
                self.inspection = Some(Inspection {
                    world: pending.world,
                    player,
                    entity,
                    tick: value["tick"].as_u64().unwrap_or(0),
                    components,
                    editable,
                    drafts: BTreeMap::new(),
                });
            }
            Phase::Fresh => {
                self.check(editor)?;
                let inspection = self
                    .inspection
                    .as_ref()
                    .ok_or("The running entity was cleared")?;
                let sets = runtime::sets(inspection, value)?;
                self.job(editor, "play.patch", json!({"sets": sets}), Phase::Patched)?;
            }
            Phase::Patched => {
                let inspection = self
                    .inspection
                    .as_mut()
                    .ok_or("The running entity was cleared")?;
                inspection.drafts.clear();
                let (player, entity) = (inspection.player, inspection.entity.clone());
                self.job(
                    editor,
                    "play.inspect",
                    json!({"player": player, "entity": entity}),
                    Phase::Refreshed,
                )?;
            }
            Phase::Refreshed => {
                let inspection = self
                    .inspection
                    .as_mut()
                    .ok_or("The running entity was cleared")?;
                inspection.components = value["components"].clone();
                inspection.tick = value["tick"].as_u64().unwrap_or(inspection.tick);
                self.message = Some(format!(
                    "Applied at tick {}. Keep Runtime Changes saves them to the scene.",
                    inspection.tick
                ));
            }
            Phase::Keep => {
                let count = |key: &str| value[key].as_array().map_or(0, Vec::len);
                self.message = Some(format!(
                    "Kept {} fields{}{}. One Undo restores the scene values.",
                    count("kept"),
                    match count("conflicts") {
                        0 => String::new(),
                        n => format!(" · {n} kept their scene edit (changed in both)"),
                    },
                    match count("skipped") {
                        0 => String::new(),
                        n => format!(" · {n} cannot be kept"),
                    },
                ));
                self.last_keep = Some(value.clone());
                self.keep = None;
            }
            Phase::Control {
                player,
                spectator,
                resume,
            } => {
                self.job(
                    editor,
                    "play.view",
                    json!({"player": player, "camera": null, "captureCurrent": spectator, "navigationSpeed": editor.state.number("viewport.cameraSpeed", 4.0)}),
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
                    self.spectators.insert(player);
                } else {
                    self.spectators.remove(&player);
                }
                if resume {
                    editor.action("play.resume", json!({"world": pending.world}))?;
                    editor.action("editor.view", json!({"play": "playing"}))?;
                }
                self.release = true;
                self.message = Some(
                    match spectator {
                        true => "Ejected: a spectator camera, the simulation running on. Click the Game view to fly it.",
                        false => "Possessed. Click the Game view to control it.",
                    }
                    .into(),
                );
            }
        }
        Ok(())
    }
}

/// Sets client `player`'s network profile (one of [`PROFILES`]) for the next Play start.
pub(crate) fn set_network_profile(
    editor: &mut Editor,
    player: usize,
    profile: &str,
) -> ara_ui::Result<()> {
    if !(1..=16).contains(&player) || !PROFILES.iter().any(|(name, _)| *name == profile) {
        return Err("Choose a supported client network profile".into());
    }
    let mut profiles = editor
        .state
        .settings
        .get("play.networkProfiles")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    profiles.insert(player.to_string(), json!(profile));
    editor.action(
        "editor.settings.set",
        json!({"key": "play.networkProfiles", "value": profiles}),
    )?;
    Ok(())
}

/// Client `player`'s network profile, `clean` when unset.
pub(crate) fn network_profile(editor: &Editor, player: usize) -> &str {
    editor
        .state
        .settings
        .get("play.networkProfiles")
        .and_then(|map| map.get(player.to_string()))
        .and_then(Value::as_str)
        .unwrap_or("clean")
}
