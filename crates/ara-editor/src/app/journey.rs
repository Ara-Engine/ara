//! `--verify-journey`: the 0.0.1-alpha's essential journey through the editor's own
//! UI paths (registry commands, the asset drag, menus, the Game view), captured at
//! each step: open a scene, import, place, make a prefab and override it, build the
//! hangar as a real game and run it headless for one frame, create a C# behaviour and
//! hot-reload it while playing in the Game view, break it and see the compile failure
//! explained in Problems, undo, save, reopen, then build again with the C# module
//! shipped (NativeAOT) and run that game too.
//! Offscreen only (hidden window). Writes `NN-step.png` per step and, after every
//! step, `journey.json` (what each step did, whether it worked and how long it took).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_editor_gpu::NativeUi;
use ara_render::Renderer;
use ara_ui_render::gpu::Renderer as UiRenderer;
use serde_json::{Value, json};

use crate::shell::commands::Target;
use crate::{Editor, shell::Shell};

/// The file the import step brings into the project (the demo's CC0 rock, ~1 m).
const IMPORT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ara-player/demo/assets/props/rock.glb"
);
/// Frames a step may wait for its effect (jobs, Play, reloads) before giving up.
const PATIENCE: u32 = 144 * 90;
/// The build step cooks and packs the whole hangar: it may take much longer.
const BUILD_PATIENCE: u32 = 144 * 900;
/// SDL scancodes the Play step types into the Game view: W walks, Esc releases.
const KEY_W: i32 = 26;
const KEY_ESCAPE: i32 = 41;
/// Editor frames the Play step holds W (the Salvager walks out of the start pose).
const WALK: u32 = 360;
/// How long a step's capture may wait for the Scene's content: captures take only
/// complete scenes, and on a fresh project the importer is still cooking models.
const CAPTURE_PATIENCE: Duration = Duration::from_secs(900);

pub(super) struct Journey {
    step: usize,
    age: u32,
    started: Instant,
    output: PathBuf,
    report: Vec<Value>,
    placed: Option<String>,
    instance: Option<String>,
    script: Option<PathBuf>,
    /// The working script while the failure step has it broken.
    source: Option<String>,
    log_cursor: u64,
    explained: bool,
    /// The frame the Play step started holding W.
    walking: Option<u32>,
    /// A label the capture hovers so its tooltip (a disabled reason) shows.
    hover: Option<&'static str>,
    note: String,
    /// The step is done and waits for a complete Scene to capture: whether it
    /// worked, and since when it waits.
    capturing: Option<(bool, Instant)>,
    /// The build folder before a build step: the step needs a new one.
    previous_build: Option<PathBuf>,
}

impl Journey {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        editor.action("editor.preferences", json!({"reduceMotion": true}))?;
        editor.action("editor.theme", json!({"light": false}))?;
        editor.action("editor.layout.reset", json!({}))?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            step: 0,
            age: 0,
            started: Instant::now(),
            output: output.into(),
            report: Vec::new(),
            placed: None,
            instance: None,
            script: None,
            source: None,
            log_cursor: 0,
            explained: false,
            walking: None,
            hover: None,
            note: String::new(),
            capturing: None,
            previous_build: None,
        })
    }

    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }

    /// Advance one frame; returns whether the UI changed.
    pub fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        if let Some((ok, _)) = self.capturing {
            return self.capture(ok, editor, shell, gpu, ui, renderer, scene_view);
        }
        self.age += 1;
        let (name, _) = STEPS[self.step];
        let ready = match self.run(editor, shell) {
            Ok(ready) => ready,
            Err(error) => {
                self.note = format!("failed: {error}");
                true
            }
        };
        let patience = if matches!(name, "export" | "export-csharp") {
            BUILD_PATIENCE
        } else {
            PATIENCE
        };
        let timed_out = self.age > patience;
        if !(ready || timed_out) {
            return Ok(self.age <= 2);
        }
        if timed_out && self.note.is_empty() {
            self.note = "timed out waiting for the step's effect".into();
        }
        let ok = !timed_out && !self.note.starts_with("failed");
        self.capturing = Some((ok, Instant::now()));
        self.capture(ok, editor, shell, gpu, ui, renderer, scene_view)
    }

    /// Capture the finished step and record it; until the Scene's content is
    /// complete (models still cooking), try again on later frames.
    #[allow(
        clippy::too_many_arguments,
        reason = "the loop's GPU owners are borrowed separately"
    )]
    fn capture(
        &mut self,
        ok: bool,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        shell.rebuild(editor)?;
        if let Some(label) = self.hover
            && let Some(rect) = shell.ui.nodes.iter().find_map(|n| {
                n.text
                    .as_ref()
                    .is_some_and(|t| t.value == label)
                    .then_some(n.rect)
            })
        {
            shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
            shell.hover_tooltip()?;
        }
        shell.upload_thumbnails(ui)?;
        let capture = super::verify::Verification::new(&self.output);
        let (name, what) = STEPS[self.step];
        let file = format!("{:02}-{name}", self.step + 1);
        if let Err(error) = capture.screenshot(&file, editor, shell, gpu, ui, renderer, scene_view)
        {
            let waited = self
                .capturing
                .map_or(Duration::ZERO, |(_, at)| at.elapsed());
            if waited < CAPTURE_PATIENCE {
                return Ok(true);
            }
            return Err(error);
        }
        self.capturing = None;
        self.hover = None;
        self.report.push(json!({
            "step": file,
            "what": what,
            "ok": ok,
            "note": self.note,
            "frames": self.age,
            "atSeconds": self.started.elapsed().as_secs_f64(),
        }));
        self.after(editor, shell)?;
        self.note.clear();
        self.step += 1;
        self.age = 0;
        std::fs::write(
            self.output.join("journey.json"),
            ara_json::to_canonical_string(&json!({
                "finished": self.finished(),
                "steps": self.report,
            })),
        )?;
        Ok(true)
    }

    /// Do (on the first frame) and check (on later frames) the current step; true
    /// when its effect is visible and the capture can be taken.
    fn run(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<bool> {
        let first = self.age == 1;
        let settled = self.age > 12;
        let name = STEPS[self.step].0;
        match name {
            "open-scene-picker" => {
                if first {
                    shell.run_command(editor, "file.open", &Target::None)?;
                }
                Ok(settled)
            }
            "scene-open" => {
                if first {
                    editor.action("editor.palette", json!({"open": false}))?;
                    let doc = editor
                        .host
                        .store()
                        .workspace()
                        .find("levels/hangar.scene")
                        .map(ToString::to_string)
                        .ok_or("the project has no levels/hangar.scene")?;
                    shell.run_command(editor, "file.open_scene", &Target::Doc(doc))?;
                    self.note = "File > Open Scene > hangar.scene".into();
                }
                Ok(settled)
            }
            "import" => {
                if first {
                    editor.action("editor.workspace", json!({"folder": ""}))?;
                    super::import_file(editor, Path::new(IMPORT))?;
                    self.note =
                        "Assets > Import… rock.glb (copied into the project, imported off-thread)"
                            .into();
                    return Ok(false);
                }
                let indexed = editor
                    .host
                    .store()
                    .workspace()
                    .index()
                    .id_of("rock.glb")
                    .is_some();
                let jobs = editor.action("jobs.list", json!({}))?;
                // This import's own job (Play may be cooking other assets meanwhile).
                let busy = jobs["jobs"].as_array().into_iter().flatten().any(|j| {
                    matches!(j["status"].as_str(), Some("running" | "queued"))
                        && j["name"].as_str().is_some_and(|n| n.contains("rock.glb"))
                });
                if indexed && !busy {
                    editor.action("editor.workspace", json!({"asset": "rock.glb"}))?;
                    shell.rebuild(editor)?;
                    return Ok(self.age > 60);
                }
                Ok(false)
            }
            "place" => {
                if first {
                    // Typed into the Assets search box: the new model's card is on screen.
                    editor.action(
                        "editor.search",
                        json!({"panel": "assets", "query": "rock.glb"}),
                    )?;
                    shell.rebuild(editor)?;
                    return Ok(false);
                }
                if self.placed.is_none() {
                    let Some(point) = shell.asset_point("rock.glb") else {
                        return Ok(false);
                    };
                    let rect = shell.viewport().ok_or("no Scene view to drop into")?;
                    shell.pointer = point;
                    shell.begin_drag();
                    shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.6];
                    if !shell.drop_at(editor)? {
                        return Err("the asset drag did not drop".into());
                    }
                    let created = newest_created(editor).ok_or("the drop created no entity")?;
                    editor.action("editor.select", json!({"entities": [created]}))?;
                    shell.run_command(editor, "view.frame", &Target::None)?;
                    self.placed = Some(created);
                    editor.action("editor.search", json!({"panel": "assets", "query": ""}))?;
                    self.note =
                        "searched Assets for rock.glb, dragged it into the Scene view".into();
                }
                Ok(self.age > 30)
            }
            "prefab" => {
                if first {
                    shell.run_command(editor, "create.prefab", &Target::None)?;
                    self.instance = editor.state.selection.first().cloned();
                    self.note = "Create > Prefab from Selection".into();
                }
                Ok(settled)
            }
            "override" => {
                if first {
                    let instance = self.instance.clone().ok_or("no prefab instance")?;
                    editor.apply(
                        vec![json!({"cmd":"property.set","args":{"entity":instance,"path":"Transform.scale","value":[1.6,1.6,1.6]}})],
                        "Scale the rock",
                        None,
                    )?;
                    shell.rebuild(editor)?;
                    shell.open_context_at(editor, "field-Transform-scale")?;
                    self.note =
                        "edited Scale in the Inspector (an override), right-clicked it".into();
                }
                Ok(settled)
            }
            "apply-override" => {
                if first {
                    shell.menu = None;
                    shell.run_command(editor, "prefab.apply_all", &Target::None)?;
                    self.note = "Inspector prefab bar > Apply all".into();
                }
                Ok(settled)
            }
            "script" => {
                if first {
                    shell.open_ide = false;
                    shell.run_command(editor, "assets.create.script", &Target::None)?;
                    let module = crate::shell::scripts::modules(editor.host.root())
                        .into_iter()
                        .next()
                        .ok_or("no C# module was created")?;
                    let folder = module
                        .strip_prefix(editor.host.root())
                        .unwrap_or(&module)
                        .to_string_lossy()
                        .replace('\\', "/");
                    self.script = Some(module.join("Spinner.cs"));
                    editor.action("editor.workspace", json!({"folder": folder}))?;
                    self.note = "Create > C# Behaviour (Spinner.cs; outside verification it opens in the IDE)".into();
                }
                Ok(settled)
            }
            "play" => {
                if first {
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.note = "Play (F5) in the Game view".into();
                    return Ok(false);
                }
                if !shell.game_live {
                    return Ok(false);
                }
                // Click the view (input capture), then hold W through the same path as
                // real keys: the Salvager walks and the follow camera comes along.
                let Some(from) = self.walking else {
                    shell.run_command(editor, "game.capture", &Target::None)?;
                    key(editor, shell, KEY_W, true);
                    self.walking = Some(self.age);
                    self.note
                        .push_str("; clicked the Game view and held W: the Salvager walks");
                    return Ok(false);
                };
                if self.age == from + WALK {
                    key(editor, shell, KEY_W, false);
                }
                Ok(self.age > from + WALK + 60)
            }
            "hot-reload" => {
                if first {
                    let logs = editor.action("logs", json!({"limit": 1}))?;
                    self.log_cursor = logs["cursor"].as_u64().unwrap_or(0);
                    let path = self.script.clone().ok_or("no script")?;
                    let source = std::fs::read_to_string(&path)?;
                    std::fs::write(&path, source.replace("90f", "240f"))?;
                    editor.action("editor.focus", json!({"panel": "console"}))?;
                    self.note = "edited Spinner.cs while playing and saved".into();
                    return Ok(false);
                }
                let logs = editor.action(
                    "logs",
                    json!({"since": self.log_cursor, "group": false, "limit": 50}),
                )?;
                let reloaded = logs["entries"].as_array().into_iter().flatten().any(|e| {
                    let text = e["diagnostic"]["message"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase();
                    text.contains("reload") || text.contains("compil")
                });
                if reloaded && !self.note.contains("Console") {
                    self.note.push_str("; the Console shows the reload");
                }
                Ok(reloaded && self.age > 30)
            }
            "failure" => {
                if first {
                    let logs = editor.action("logs", json!({"limit": 1}))?;
                    self.log_cursor = logs["cursor"].as_u64().unwrap_or(0);
                    let path = self.script.clone().ok_or("no script")?;
                    let source = std::fs::read_to_string(&path)?;
                    // A missing semicolon: the hot reload fails to compile.
                    std::fs::write(&path, source.replacen(';', "", 1))?;
                    self.source = Some(source);
                    self.note = "deleted a semicolon in Spinner.cs while playing".into();
                    return Ok(false);
                }
                let errors = editor.action(
                    "logs",
                    json!({"since": self.log_cursor, "severity": "error", "group": false, "limit": 20}),
                )?;
                if errors["entries"].as_array().is_none_or(Vec::is_empty) {
                    return Ok(false);
                }
                if !self.explained {
                    self.explained = true;
                    editor.action("editor.layout.show", json!({"panel": "problems"}))?;
                    shell.rebuild(editor)?;
                    let seq = shell
                        .ui
                        .nodes
                        .iter()
                        .filter_map(|n| n.element.id.strip_prefix("problem-"))
                        .filter_map(|s| s.parse::<u64>().ok())
                        .max()
                        .ok_or("Problems shows nothing")?;
                    shell.run_command(
                        editor,
                        "problems.explain",
                        &Target::Text(seq.to_string()),
                    )?;
                    self.note
                        .push_str("; the compile error is in Problems, Why explains it");
                }
                Ok(self.age > 30)
            }
            "stop" => {
                if first {
                    if let (Some(path), Some(source)) = (&self.script, self.source.take()) {
                        std::fs::write(path, source)?;
                    }
                    shell.run_command(editor, "play.stop", &Target::None)?;
                    editor.action("editor.focus", json!({"panel": "scene"}))?;
                    self.note = "fixed the script, stopped Play".into();
                }
                Ok(!shell.game_live && settled)
            }
            "undo" => {
                if first {
                    let entity = self.instance.clone().ok_or("no prefab instance")?;
                    editor.apply(
                        vec![json!({"cmd":"transform.set","args":{"entities":[entity],"position":[2.0,0.0,-6.0]}})],
                        "Move the rock",
                        None,
                    )?;
                    shell.open_bar(1, false);
                    self.note = "moved the rock; the Edit menu's Undo names the move".into();
                }
                Ok(settled)
            }
            "save-reopen" => {
                if first {
                    shell.menu = None;
                    shell.run_command(editor, "edit.undo", &Target::None)?;
                    shell.run_command(editor, "file.save", &Target::None)?;
                    let main = editor
                        .host
                        .store()
                        .workspace()
                        .find("levels/outpost.scene")
                        .map(ToString::to_string)
                        .ok_or("no levels/outpost.scene")?;
                    shell.run_command(editor, "file.open_scene", &Target::Doc(main))?;
                    let hangar = editor
                        .host
                        .store()
                        .workspace()
                        .find("levels/hangar.scene")
                        .map(ToString::to_string)
                        .ok_or("no levels/hangar.scene")?;
                    shell.run_command(editor, "file.open_scene", &Target::Doc(hangar))?;
                    editor.action("editor.focus", json!({"panel": "scene"}))?;
                    self.note = "Undo, Save, open outpost.scene, reopen hangar.scene (its camera comes back)".into();
                }
                Ok(settled)
            }
            "export" | "export-csharp" => {
                if first {
                    if name == "export" {
                        // The hangar is the game: Project Settings > Project > Main Scene.
                        editor.action(
                            "editor.settings.set",
                            json!({"key": "project.mainScene", "value": "levels/hangar.scene"}),
                        )?;
                        self.note = "Project Settings > Main Scene = hangar.scene, then Build > Build Current Profile (no profile yet: it creates presets/windows.export.json)".into();
                    } else {
                        self.note = "Build > Build Current Profile again: the project now has a C# module, which the export publishes NativeAOT".into();
                    }
                    self.previous_build = shell.last_build.clone();
                    shell.run_command(editor, "build.current", &Target::None)?;
                    editor.action("editor.focus", json!({"panel": "console"}))?;
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
                if name == "export-csharp" && !game.join("game/scripts.json").is_file() {
                    return Err(
                        "the build did not ship the C# module (no game/scripts.json)".into(),
                    );
                }
                if !self.note.contains("builds/") {
                    let shown = game
                        .strip_prefix(dunce::simplified(editor.host.root()))
                        .unwrap_or(&game)
                        .to_string_lossy()
                        .replace('\\', "/");
                    self.note.push_str(&format!("; the game is in {shown}"));
                }
                Ok(self.age > 30)
            }
            "run-build" | "run-build-csharp" => {
                if first {
                    shell.run_command(editor, "build.run", &Target::None)?;
                    self.note = "Build > Run Build (offscreen: the game runs headless for one frame and captures it)".into();
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
                    .join(format!("{:02}-{name}-game.png", self.step + 1));
                if !copy.exists() {
                    std::fs::copy(&capture, &copy)?;
                    self.note.push_str(&format!(
                        "; its frame is {}",
                        copy.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                Ok(self.age > 30)
            }
            _ => Ok(true),
        }
    }

    /// Clean up after a capture.
    fn after(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if STEPS[self.step].0 == "undo" {
            shell.menu = None;
            shell.rebuild(editor)?;
        }
        if STEPS[self.step].0 == "play" && shell.game_captured {
            key(editor, shell, KEY_ESCAPE, true);
        }
        Ok(())
    }
}

/// A key event through the Game view's input routing, as the window would send it.
fn key(editor: &mut Editor, shell: &mut Shell, scancode: i32, pressed: bool) {
    let event = ara_platform::Event::Key {
        scancode,
        pressed,
        repeat: false,
    };
    super::game_input(&event, editor, shell, 1.0);
}

/// The newest entity created by a human edit.
fn newest_created(editor: &Editor) -> Option<String> {
    editor.host.store().history().iter().rev().find_map(|t| {
        t.ops.iter().find_map(|op| match op {
            ara_doc::Op::Create { id, .. } => Some(id.to_string()),
            _ => None,
        })
    })
}

/// The journey, in order: (step, what it shows).
const STEPS: &[(&str, &str)] = &[
    (
        "open-scene-picker",
        "File > Open Scene lists every scene, recent first",
    ),
    ("scene-open", "the hangar scene opens, framed"),
    (
        "import",
        "an imported glTF with its thumbnail and import settings",
    ),
    ("place", "the asset dragged into the Scene view"),
    (
        "prefab",
        "Prefab from Selection: a linked instance and the prefab bar",
    ),
    (
        "override",
        "an override (1 override) and the field menu to apply it",
    ),
    (
        "apply-override",
        "Apply all writes the override into the prefab",
    ),
    (
        "export",
        "Build Current Profile exports the hangar as a real game",
    ),
    (
        "run-build",
        "Run Build: the exported game runs headless for one frame",
    ),
    ("script", "a C# behaviour created in the project's module"),
    ("play", "Play inside the Game view, walking with W"),
    ("hot-reload", "the edited script hot-reloads while playing"),
    (
        "failure",
        "a broken script: the compile error in Problems with its why",
    ),
    ("stop", "the script fixed, Play stopped"),
    ("undo", "Edit > Undo names the change"),
    (
        "save-reopen",
        "saved, another scene opened, the hangar reopened",
    ),
    (
        "export-csharp",
        "Build Current Profile again ships the C# module (NativeAOT)",
    ),
    (
        "run-build-csharp",
        "Run Build: the game with its C# runs headless for one frame",
    ),
];
