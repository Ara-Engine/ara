//! `--verify-first-hour`: a new user's first hour on a freshly created project, replayed
//! through the editor's own UI paths and captured as the user sees it (the presented
//! frame, not a settled Scene capture). The project opens cold, so its first steps run
//! while imports are still cooking: the loading look, the Importing chip and Scene
//! notice, browsing, searching and picking in Assets. Then the imports finish and the
//! session works the project: Material Overrides, colours and sliders, undo and redo,
//! the Material, VFX and Animation pages, Play / Stop / Play again (background
//! progress), Build Current Profile and Run Build, and finally Problems and Console.
//! Offscreen only. Writes `NN-step.png` per step and, after every step,
//! `first-hour.json`: what each step did and found, how long it took, its frame-time
//! p95/max, and every Problems entry at the end.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_editor_gpu::NativeUi;
use serde_json::{Value, json};

use crate::shell::commands::Target;
use crate::{Editor, shell::Shell};

/// Frames a step may wait for its effect (about a minute at 144 Hz).
const PATIENCE: u32 = 144 * 60;
/// Imports, Play and builds may take much longer on a cold project.
const LONG_PATIENCE: Duration = Duration::from_secs(1500);

const STEPS: &[(&str, &str)] = &[
    (
        "cold-open",
        "the project opens cold while its imports run: loading look, Importing chip, Scene notice",
    ),
    (
        "assets-browse",
        "Assets: the folder with the most models, thumbnails and import badges while importing",
    ),
    ("assets-search", "the Assets search finds models by name"),
    (
        "pick-model",
        "an entity's Model field opens a picker of the project's models",
    ),
    (
        "imports-done",
        "every import has finished: the Scene shows the art",
    ),
    (
        "material-overrides",
        "Material Overrides: Add picks a material and makes a slot",
    ),
    (
        "colours",
        "colour (hex), emissive colour and roughness edited through their widgets",
    ),
    (
        "undo-redo",
        "Undo walks the edits back, Redo brings one back",
    ),
    ("material-page", "the Material page opens a material graph"),
    (
        "vfx-page",
        "Create > VFX System makes an effect and opens the VFX page",
    ),
    (
        "animation-page",
        "the Animation page opens the character controller",
    ),
    (
        "play-starting",
        "Play starts in the background: the Game view shows its progress",
    ),
    ("play-running", "the game runs in the Game view"),
    ("play-again", "Stop, then Play again"),
    ("build", "Build > Build Current Profile exports the game"),
    ("run-build", "Build > Run Build runs the exported game"),
    ("problems", "Problems after the session"),
    ("console", "Console after the session"),
];

/// Text typed into inspector fields through the real field input path.
#[derive(Default)]
struct Clip(String);
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}

/// Frame times observed during one step.
#[derive(Default)]
struct Frames {
    cpu: Vec<f64>,
    tick: Vec<f64>,
    view: Vec<f64>,
}
impl Frames {
    fn summary(&mut self) -> Value {
        let stats = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let at = |q: f64| v.get(((v.len() as f64 - 1.) * q).round() as usize).copied();
            json!({"p50": at(0.5), "p95": at(0.95), "max": v.last(), "over16_7": v.iter().filter(|&&ms| ms > 16.7).count()})
        };
        json!({"frames": self.cpu.len(), "cpuMs": stats(&mut self.cpu), "tickMs": stats(&mut self.tick), "viewMs": stats(&mut self.view)})
    }
}

pub(super) struct FirstHour {
    step: usize,
    age: u32,
    started: Instant,
    step_started: Instant,
    output: PathBuf,
    report: Vec<Value>,
    note: String,
    found: serde_json::Map<String, Value>,
    frames: Frames,
    /// The step is done: capture the next presented frame.
    capture: Option<bool>,
    /// The entity the session edits (one with a model).
    entity: Option<String>,
    clip: Clip,
    previous_build: Option<PathBuf>,
}

impl FirstHour {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        editor.action("editor.theme", json!({"light": false}))?;
        editor.action("editor.layout.reset", json!({}))?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            step: 0,
            age: 0,
            started: Instant::now(),
            step_started: Instant::now(),
            output: output.into(),
            report: Vec::new(),
            note: String::new(),
            found: Default::default(),
            frames: Frames::default(),
            capture: None,
            entity: None,
            clip: Clip::default(),
            previous_build: None,
        })
    }

    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }

    /// Record one frame's costs against the current step.
    pub fn frame(&mut self, cpu_ms: f64, tick_ms: f64, view_ms: f64) {
        self.frames.cpu.push(cpu_ms);
        self.frames.tick.push(tick_ms);
        self.frames.view.push(view_ms);
    }

    /// Advance one frame; returns whether the UI changed.
    pub fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        // The frame presented after the step settled is what the user saw.
        if let Some(ok) = self.capture.take() {
            return self.record(ok, editor, shell, gpu);
        }
        self.age += 1;
        let ready = match self.run(editor, shell) {
            Ok(ready) => ready,
            Err(error) => {
                self.note = format!("failed: {error}");
                true
            }
        };
        let name = STEPS[self.step].0;
        let long = matches!(
            name,
            "imports-done" | "play-running" | "play-again" | "build" | "run-build"
        );
        let timed_out = if long {
            self.step_started.elapsed() > LONG_PATIENCE
        } else {
            self.age > PATIENCE
        };
        if !(ready || timed_out) {
            return Ok(self.age <= 2);
        }
        if timed_out && !self.note.starts_with("failed") {
            self.note = format!("timed out: {}", self.note);
        }
        self.capture = Some(!timed_out && !self.note.starts_with("failed"));
        Ok(true)
    }

    fn record(
        &mut self,
        ok: bool,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<bool> {
        let (name, what) = STEPS[self.step];
        let file = format!("{:02}-{name}", self.step + 1);
        gpu.pixels()?
            .save(self.output.join(format!("{file}.png")))?;
        let mut found = std::mem::take(&mut self.found);
        if let Some((pending, failed, total)) = shell.import_counts() {
            found.insert(
                "imports".into(),
                json!({"pending": pending, "failed": failed, "total": total}),
            );
        }
        self.report.push(json!({
            "step": file,
            "what": what,
            "ok": ok,
            "note": self.note,
            "found": found,
            "seconds": self.step_started.elapsed().as_secs_f64(),
            "atSeconds": self.started.elapsed().as_secs_f64(),
            "frameTimes": self.frames.summary(),
        }));
        self.after(editor, shell)?;
        self.note.clear();
        self.frames = Frames::default();
        self.step += 1;
        self.age = 0;
        self.step_started = Instant::now();
        let problems = if self.finished() {
            editor.action(
                "logs",
                json!({"severity": "warning", "group": true, "limit": 200}),
            )?
        } else {
            Value::Null
        };
        std::fs::write(
            self.output.join("first-hour.json"),
            ara_json::to_canonical_string(&json!({
                "finished": self.finished(),
                "steps": self.report,
                "problems": problems,
            })),
        )?;
        Ok(true)
    }

    /// Types `text` into the field `id` and commits it, as Enter does.
    fn type_into(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        id: &str,
        text: &str,
    ) -> ara_ui::Result<()> {
        use ara_ui::input::InputEvent;
        use ara_ui_text::EditEvent;
        // Scroll the field's row into view first: off-screen controls do not hit.
        let path = id.trim_start_matches("edit-").replacen('-', ".", 1);
        shell.rebuild(editor)?;
        shell.reveal_field(&path)?;
        shell.frame()?;
        shell
            .click(id, &mut self.clip)
            .map_err(|e| format!("{id}: {e}"))?;
        shell.event(InputEvent::Edit(EditEvent::SelectAll), &mut self.clip)?;
        shell.event(
            InputEvent::Edit(EditEvent::Insert(text.into())),
            &mut self.clip,
        )?;
        shell.dispatch(editor)?;
        shell.commit_fields(editor)?;
        Ok(())
    }

    /// The open menu's rows (node ID, label) whose label ends with `suffix`.
    fn menu_rows(shell: &Shell, suffix: &str) -> Vec<(String, String)> {
        shell
            .ui
            .nodes
            .iter()
            .filter(|n| n.element.id.starts_with("m0-") && n.element.id.ends_with("-label"))
            .filter_map(|n| {
                let label = n.text.as_ref()?.value.clone();
                label
                    .ends_with(suffix)
                    .then(|| (n.element.id.trim_end_matches("-label").to_owned(), label))
            })
            .collect()
    }

    /// Clicks the control `id` and dispatches it.
    fn press(&mut self, editor: &mut Editor, shell: &mut Shell, id: &str) -> ara_ui::Result<()> {
        shell.rebuild(editor)?;
        shell
            .click(id, &mut self.clip)
            .map_err(|e| format!("{id}: {e}"))?;
        shell.dispatch(editor)?;
        Ok(())
    }

    /// The selected entity's effective `MeshRenderer.field`.
    fn mesh_field(&self, editor: &Editor, field: &str) -> Value {
        let Some(entity) = &self.entity else {
            return Value::Null;
        };
        crate::world::resolve(editor.host.store(), &editor.state.doc)
            .ok()
            .and_then(|r| {
                r.get(entity)
                    .and_then(|e| e.record.get("MeshRenderer"))
                    .cloned()
            })
            .and_then(|m| m.get(field).cloned())
            .unwrap_or(Value::Null)
    }

    /// Do (on the first frame) and check (on later frames) the current step.
    fn run(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        let first = self.age == 1;
        let settled = self.age > 24;
        match STEPS[self.step].0 {
            "cold-open" => {
                let chip = shell.ui.find("status-imports").is_some();
                let notice = shell.ui.find("scene-loading").is_some();
                self.found.insert("importingChip".into(), json!(chip));
                self.found.insert("sceneNotice".into(), json!(notice));
                // A second after the window is up, while the first imports cook.
                Ok(self.age > 144)
            }
            "assets-browse" => {
                if first {
                    let folder = shell.model_folder().ok_or("no models in the project")?;
                    editor.action("editor.layout.show", json!({"panel": "assets"}))?;
                    editor.action("editor.workspace", json!({"folder": folder}))?;
                    self.note = format!("Assets > {folder}");
                }
                let badges = shell
                    .ui
                    .nodes
                    .iter()
                    .filter(|n| n.element.id.ends_with("-import"))
                    .count();
                let thumbnails = shell
                    .ui
                    .nodes
                    .iter()
                    .filter(|n| n.element.id.ends_with("-thumbnail"))
                    .count();
                self.found.insert("importBadges".into(), json!(badges));
                self.found.insert("thumbnails".into(), json!(thumbnails));
                Ok(self.age > 288)
            }
            "assets-search" => {
                if first {
                    let query = shell.model_query().ok_or("no model to search for")?;
                    editor.action("editor.search", json!({"panel": "assets", "query": query}))?;
                    self.note = format!("searched Assets for `{query}`");
                }
                let count = shell
                    .ui
                    .find("assets-count")
                    .and_then(|i| shell.ui.nodes[i].text.as_ref())
                    .map(|t| t.value.clone());
                self.found.insert("results".into(), json!(count));
                Ok(settled)
            }
            "pick-model" => {
                if first {
                    editor.action("editor.search", json!({"panel": "assets", "query": ""}))?;
                    let entity = crate::world::resolve(editor.host.store(), &editor.state.doc)
                        .ok()
                        .and_then(|r| {
                            r.iter().find_map(|(id, e)| {
                                e.record
                                    .get("MeshRenderer")
                                    .and_then(|m| m.get("model"))
                                    .and_then(Value::as_str)
                                    .is_some()
                                    .then(|| id.clone())
                            })
                        })
                        .ok_or("no entity with a model")?;
                    editor.action("editor.select", json!({"entities": [entity]}))?;
                    editor.action("editor.layout.show", json!({"panel": "inspector"}))?;
                    self.entity = Some(entity.clone());
                    self.note = format!("selected {entity}, opened its Model picker");
                    return Ok(false);
                }
                if self.age == 6 {
                    self.press(editor, shell, "asset-field-MeshRenderer-model")?;
                }
                let rows = Self::menu_rows(shell, "");
                self.found.insert(
                    "pickerRows".into(),
                    json!(rows.iter().map(|(_, label)| label).collect::<Vec<_>>()),
                );
                Ok(self.age > 30 && shell.menu.is_some())
            }
            "imports-done" => {
                if first {
                    self.note = "waited for every import".into();
                }
                Ok(shell
                    .import_counts()
                    .is_some_and(|(pending, _, _)| pending == 0)
                    && self.age > 60)
            }
            "material-overrides" => {
                if first {
                    editor.action("editor.layout.show", json!({"panel": "inspector"}))?;
                    return Ok(false);
                }
                if self.age == 6 {
                    self.press(editor, shell, "add-MeshRenderer-materialOverrides")?;
                }
                if self.age == 30 {
                    let row = Self::menu_rows(shell, ".graph")
                        .into_iter()
                        .next()
                        .map(|(id, _)| id)
                        .ok_or("the picker offers no material")?;
                    self.press(editor, shell, &row)?;
                    self.note = "Material overrides > Add > picked a material".into();
                }
                let slots = self.mesh_field(editor, "materialOverrides");
                self.found.insert("materialOverrides".into(), slots.clone());
                Ok(self.age > 54 && slots.as_array().is_some_and(|s| !s.is_empty()))
            }
            "colours" => {
                if first {
                    self.type_into(editor, shell, "edit-MeshRenderer-color", "#C0392B")?;
                    self.type_into(editor, shell, "edit-MeshRenderer-emissive", "#FF8800")?;
                    self.type_into(editor, shell, "edit-MeshRenderer-roughness", "0.25")?;
                    self.note = "typed colour #C0392B, emissive #FF8800, roughness 0.25".into();
                }
                for field in ["color", "emissive", "roughness"] {
                    self.found
                        .insert(field.into(), self.mesh_field(editor, field));
                }
                Ok(settled)
            }
            "undo-redo" => {
                if first {
                    for _ in 0..4 {
                        shell.run_command(editor, "edit.undo", &Target::None)?;
                    }
                    shell.run_command(editor, "edit.redo", &Target::None)?;
                    self.note =
                        "Undo ×4 (roughness, emissive, colour, the slot), Redo ×1 (the slot)"
                            .into();
                }
                for field in ["color", "emissive", "roughness", "materialOverrides"] {
                    self.found
                        .insert(field.into(), self.mesh_field(editor, field));
                }
                Ok(settled)
            }
            "material-page" => {
                if first {
                    let doc = editor
                        .host
                        .store()
                        .workspace()
                        .docs()
                        .find(|(_, d)| {
                            d.record("g_meta")
                                .and_then(|r| r.get("domain"))
                                .and_then(Value::as_str)
                                == Some("material")
                        })
                        .map(|(_, d)| d.id.to_string())
                        .ok_or("no material graph")?;
                    editor.action("editor.workspace", json!({"resource": doc, "asset": ""}))?;
                    editor.action("editor.layout.show", json!({"panel": "material"}))?;
                    self.note = format!("opened {doc}");
                }
                Ok(self.age > 240)
            }
            "vfx-page" => {
                if first {
                    shell.run_command(editor, "assets.create.vfx", &Target::None)?;
                    editor.action("editor.layout.show", json!({"panel": "vfx"}))?;
                }
                Ok(self.age > 240)
            }
            "animation-page" => {
                if first {
                    let doc = editor
                        .host
                        .store()
                        .workspace()
                        .docs()
                        .find(|(_, d)| d.kind == "animation")
                        .map(|(_, d)| d.id.to_string())
                        .ok_or("no animation controller")?;
                    editor.action("editor.workspace", json!({"resource": doc, "asset": ""}))?;
                    editor.action("editor.layout.show", json!({"panel": "animation"}))?;
                    self.note = format!("opened {doc}");
                }
                Ok(self.age > 240)
            }
            "play-starting" => {
                if first {
                    editor.action("editor.focus", json!({"panel": "scene"}))?;
                    let clicked = Instant::now();
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.found.insert(
                        "clickMs".into(),
                        json!(clicked.elapsed().as_secs_f64() * 1000.),
                    );
                    return Ok(false);
                }
                Ok(self.age > 12)
            }
            "play-running" => {
                self.found.insert("gameLive".into(), json!(shell.game_live));
                Ok(shell.game_live && self.age > 60)
            }
            "play-again" => {
                if first {
                    shell.run_command(editor, "play.stop", &Target::None)?;
                    return Ok(false);
                }
                if self.age == 60 {
                    shell.run_command(editor, "play.start", &Target::None)?;
                }
                Ok(self.age > 60 && shell.game_live && self.age > 120)
            }
            "build" => {
                if first {
                    if editor.state.playing.is_some() {
                        shell.run_command(editor, "play.stop", &Target::None)?;
                    }
                    self.previous_build = shell.last_build.clone();
                    shell.run_command(editor, "build.current", &Target::None)?;
                    return Ok(false);
                }
                if shell.build.is_some() {
                    return Ok(false);
                }
                let game = shell
                    .last_build
                    .clone()
                    .filter(|game| Some(game) != self.previous_build.as_ref())
                    .ok_or("the build failed; see Problems")?;
                self.found
                    .insert("build".into(), json!(game.display().to_string()));
                Ok(self.age > 30)
            }
            "run-build" => {
                if first {
                    shell.run_command(editor, "build.run", &Target::None)?;
                    return Ok(false);
                }
                if shell.run.is_some() {
                    return Ok(false);
                }
                let capture = shell
                    .last_run
                    .clone()
                    .ok_or("the build did not run; see Problems")?;
                let copy = self
                    .output
                    .join(format!("{:02}-run-build-game.png", self.step + 1));
                if !copy.exists() {
                    std::fs::copy(&capture, &copy)?;
                }
                Ok(self.age > 30)
            }
            "problems" => {
                if first {
                    shell.run_command(editor, "window.problems", &Target::None)?;
                }
                Ok(settled)
            }
            "console" => {
                if first {
                    shell.run_command(editor, "window.console", &Target::None)?;
                }
                Ok(settled)
            }
            _ => Ok(true),
        }
    }

    /// Clean up after a capture.
    fn after(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if matches!(STEPS[self.step].0, "pick-model" | "material-overrides") {
            shell.menu = None;
        }
        if STEPS[self.step].0 == "play-again" {
            shell.run_command(editor, "play.stop", &Target::None)?;
        }
        Ok(())
    }
}
