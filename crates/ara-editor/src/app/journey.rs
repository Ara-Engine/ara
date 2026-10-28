//! `--verify-journey`: the 0.0.1-alpha's essential journey through the editor's own
//! UI paths (registry commands, the asset drag, menus, the Game view), captured at
//! each step: open a scene, import, place, make a prefab and override it, build the
//! hangar as a real game and run it headless for one frame, create a C# behaviour,
//! see the hangar through its gameplay camera in the Game view before Play (compared
//! with Play's first frames), hot-reload it while playing, break it and see the compile failure
//! explained in Problems, undo, save, reopen, then build again with the C# module
//! shipped (NativeAOT) and run that game too.
//! Offscreen only (hidden window). Writes `NN-step.png` per step and, after every
//! step, `journey.json` (what each step did, whether it worked and how long it took).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_editor_gpu::NativeUi;
use ara_render::Renderer;
use ara_ui::paint::Box2;
use ara_ui_render::{atlas::TextMode, gpu::Renderer as UiRenderer};
use serde_json::{Value, json};

use crate::shell::commands::Target;
use crate::{Editor, shell::Shell};

/// The file the import step brings into the project (the demo's CC0 rock, ~1 m).
const IMPORT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ara-player/demo/content/assets/props/rock.glb"
);
/// Frames a step may wait for its effect (jobs, Play, reloads) before giving up.
const PATIENCE: u32 = 144 * 90;
/// The build steps cook and pack the whole hangar, and the import step waits behind the
/// project's cold imports: they may take much longer.
const BUILD_PATIENCE: u32 = 144 * 900;
/// SDL scancodes the Play step types into the Game view: W walks, Esc releases.
const KEY_W: i32 = 26;
const KEY_ESCAPE: i32 = 41;
/// Editor frames the Play step holds W (the Salvager walks out of the start pose).
const WALK: u32 = 360;
/// How long a step's capture may wait for the Scene's content: captures take only
/// complete scenes, and on a fresh project the importer is still cooking models.
const CAPTURE_PATIENCE: Duration = Duration::from_secs(900);
/// The 1-SSIM within which the Game view's camera preview counts as the view Play
/// starts with. Measured on the hangar: 0.22 for the preview against Play's first
/// frames (the Salvager animates out of its bind pose, exposure adapts), 0.58 against
/// another camera or the same view shifted by 48 px.
const PREVIEW_MATCH: f64 = 0.35;

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
    /// The frame a Stop step clicked Stop.
    stopped: Option<u32>,
    /// The frames the camera preview, then Play's first frame, showed in the Game view.
    preview_seen: Option<u32>,
    live_at: Option<u32>,
    /// The preview step's capture and its Game frame (logical), for the comparison.
    preview_frame: Option<(PathBuf, Box2)>,
    /// The frame the floating Game window first showed its preview.
    float_seen: Option<u32>,
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
            stopped: None,
            preview_seen: None,
            live_at: None,
            preview_frame: None,
            float_seen: None,
        })
    }

    pub fn finished(&self) -> bool {
        self.step >= STEPS.len()
    }

    /// Advance one frame; returns whether the UI changed.
    #[allow(
        clippy::too_many_arguments,
        reason = "the loop's GPU owners are borrowed separately"
    )]
    pub fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
        floats: &super::windows::Floats,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        if let Some((ok, _)) = self.capturing {
            return self.capture(ok, editor, shell, gpu, ui, renderer, scene_view, floats);
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
        // An import on a cold project queues behind the project's own imports; patience
        // counts frames, so a faster editor would otherwise give up sooner.
        let patience = if matches!(name, "import" | "export" | "export-csharp") {
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
        self.capture(ok, editor, shell, gpu, ui, renderer, scene_view, floats)
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
        floats: &super::windows::Floats,
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
        // The floating Game window's own image (`NN-step-window.png`).
        if matches!(name, "game-float" | "play-float") {
            let id = shell
                .game_host()
                .and_then(|(host, _)| host.map(str::to_owned))
                .ok_or("the Game view is not in a floating window")?;
            // Its OS window opens a few frames after the layout change.
            let Some(float) = floats.windows.iter().find(|f| f.id == id) else {
                let waited = self
                    .capturing
                    .map_or(Duration::ZERO, |(_, at)| at.elapsed());
                if waited < CAPTURE_PATIENCE {
                    return Ok(true);
                }
                return Err("the floating Game window never opened".into());
            };
            shell.frame_surface(&id)?;
            let surface = shell.surface(&id).ok_or("floating surface")?;
            gpu.select_target(float.key);
            ui.render(
                gpu,
                &surface.ui.display,
                &shell.text.assets,
                surface.ui.size.map(|v| v.round() as u32),
                TextMode::Raster,
            )?;
            gpu.pixels()?
                .save(self.output.join(format!("{file}-window.png")))?;
            gpu.select_target(0);
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
                    // The Assets tab (it expands the collapsed bottom bar), then typed into
                    // its search box: the new model's card is on screen.
                    editor.action("editor.layout.show", json!({"panel": "assets"}))?;
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
                        "opened Assets, searched for rock.glb, dragged it into the Scene view"
                            .into();
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
                    let mut clipboard = super::assets_verify::Clip::default();
                    shell.event(
                        ara_ui::input::InputEvent::Edit(ara_ui_text::EditEvent::Insert(
                            "Spinner".into(),
                        )),
                        &mut clipboard,
                    )?;
                    shell.dispatch(editor)?;
                    shell.key(editor, 40, true, &mut clipboard)?;
                    self.script = editor
                        .state
                        .asset_path
                        .as_deref()
                        .map(|p| editor.host.root().join(p));
                    self.note =
                        "Create > C# Script, inline rename to Spinner.cs, background compile"
                            .into();
                }
                Ok(settled && editor.host.store().schema().get("Spinner").is_some())
            }
            "stop-starting" => {
                if first {
                    self.stopped = None;
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.note = "Play, then Stop while it is still starting".into();
                    return Ok(false);
                }
                if self.stopped.is_none() {
                    if shell.game_live {
                        return Err("Play went live before Stop could cancel it".into());
                    }
                    // The toolbar's Stop: it must be there and enabled while starting.
                    if shell.shown_node("stop").is_none() {
                        return Err("the toolbar shows no Stop while Play starts".into());
                    }
                    shell.run_command(editor, "play.stop", &Target::None)?;
                    self.stopped = Some(self.age);
                    return Ok(false);
                }
                let stopped = editor.state.playing.is_none()
                    && editor.play_world.is_none()
                    && !shell.game_live
                    && shell.shown_node("play").is_some();
                const BACK: &str = "; the start was cancelled and Play is back";
                if stopped && !self.note.ends_with(BACK) {
                    self.note.push_str(BACK);
                }
                Ok(stopped && settled)
            }
            "stop-window" => {
                if first {
                    self.stopped = None;
                    editor.action(
                        "editor.settings.set",
                        json!({"key": "play.inGameView", "value": false}),
                    )?;
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.note =
                        "Play in a player window (hidden in verification), then Stop".into();
                    return Ok(false);
                }
                if self.stopped.is_none() {
                    let status = editor.action("play.status", json!({}))?;
                    let live = status["worlds"].as_array().is_some_and(|worlds| {
                        worlds.iter().any(|w| {
                            w["world"].as_str() == editor.play_world.as_deref()
                                && w["status"] == "running"
                        })
                    });
                    if live {
                        shell.run_command(editor, "play.stop", &Target::None)?;
                        self.stopped = Some(self.age);
                    }
                    return Ok(false);
                }
                const ENDED: &str = "; Stop ended the windowed world";
                let stopped = editor.state.playing.is_none() && editor.play_world.is_none();
                if stopped && !self.note.ends_with(ENDED) {
                    editor.action(
                        "editor.settings.set",
                        json!({"key": "play.inGameView", "value": true}),
                    )?;
                    self.note.push_str(ENDED);
                }
                Ok(stopped && settled)
            }
            "game-preview" => {
                if first {
                    editor.action("editor.layout.show", json!({"panel": "game"}))?;
                    self.note =
                        "the Game tab before Play: the hangar through its follow camera".into();
                    return Ok(false);
                }
                if shell.game_preview.is_none() {
                    return Ok(false);
                }
                // A few frames more: temporal history settles.
                let seen = *self.preview_seen.get_or_insert(self.age);
                Ok(self.age > seen + 90)
            }
            "game-float" => {
                if first {
                    editor.action(
                        "editor.layout.move",
                        json!({"panel": "game", "slot": "float", "rect": [160, 120, 960, 600]}),
                    )?;
                    self.note =
                        "Game tab torn into its own window: the camera preview renders there"
                            .into();
                    return Ok(false);
                }
                let floating = shell.game_host().is_some_and(|(host, _)| host.is_some());
                if !floating || shell.game_preview.is_none() {
                    return Ok(false);
                }
                let seen = *self.float_seen.get_or_insert(self.age);
                Ok(self.age > seen + 90)
            }
            "play-float" => {
                if first {
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.note = "Play in the floating Game window, at its size".into();
                    return Ok(false);
                }
                Ok(shell.game_live && settled)
            }
            "game-dock" => {
                if first {
                    shell.run_command(editor, "play.stop", &Target::None)?;
                    editor.action(
                        "editor.layout.move",
                        json!({"panel": "game", "slot": "view"}),
                    )?;
                    self.note = "Stop, and the Game tab docks back beside the Scene".into();
                    return Ok(false);
                }
                let docked = shell.game_host().is_some_and(|(host, _)| host.is_none());
                if !(docked && editor.state.playing.is_none() && !shell.game_live && settled) {
                    return Ok(false);
                }
                // Every Play → Stop → Play so far: nothing read a world that wasn't running.
                let unknown = super::qa::unknown_world_errors(editor)?;
                if unknown > 0 {
                    return Err(format!(
                        "{unknown} ARA7302 errors: the editor asked a Play world that was not running"
                    )
                    .into());
                }
                Ok(true)
            }
            "play-start" => {
                if first {
                    shell.run_command(editor, "play.start", &Target::None)?;
                    self.note = "Play (F5) in the Game view".into();
                    return Ok(false);
                }
                if !shell.game_live {
                    return Ok(false);
                }
                let live = *self.live_at.get_or_insert(self.age);
                Ok(self.age > live + 12)
            }
            "play" => {
                if !shell.game_live {
                    return Ok(false);
                }
                // Click the view (input capture), then hold W through the same path as
                // real keys: the Salvager walks and the follow camera comes along.
                let Some(from) = self.walking else {
                    shell.run_command(editor, "game.capture", &Target::None)?;
                    key(editor, shell, KEY_W, true);
                    self.walking = Some(self.age);
                    self.note = "clicked the Game view and held W: the Salvager walks".into();
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
                    std::fs::write(
                        &path,
                        source.replace("Speed * tick.Dt", "(Speed + 240f) * tick.Dt"),
                    )?;
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
                        self.note = "Project Settings > Main Scene = hangar.scene, then Build > Build Current Profile (no profile yet: it creates settings/builds/windows.export.json)".into();
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
                if name == "export-csharp"
                    && !crate::shell::build::scripts_manifest(&game)?.is_file()
                {
                    return Err(
                        "the build did not ship the C# module (no scripts manifest in application data)".into(),
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
        let name = STEPS[self.step].0;
        let capture = self.output.join(format!("{:02}-{name}.png", self.step + 1));
        let frame = shell.ui.find("game-frame").map(|i| shell.ui.nodes[i].rect);
        if name == "game-preview" {
            self.preview_frame = frame.map(|rect| (capture.clone(), rect));
        }
        if name == "play-start"
            && let (Some((preview, before)), Some(after)) = (self.preview_frame.clone(), frame)
        {
            let scale = image::image_dimensions(&capture)
                .map(|(w, _)| w as f32 / shell.ui.size[0])
                .unwrap_or(1.);
            let score = game_frames_differ((&preview, before), (&capture, after), scale)?;
            let matches = score <= PREVIEW_MATCH;
            if let Some(entry) = self.report.last_mut() {
                entry["previewVsPlay"] = json!(score);
                entry["note"] = json!(format!(
                    "{}; the camera preview before Play and Play's first frames differ by 1-SSIM {score:.3} (at most {PREVIEW_MATCH})",
                    self.note
                ));
                if !matches {
                    entry["ok"] = json!(false);
                }
            }
        }
        if name == "undo" {
            shell.menu = None;
            shell.rebuild(editor)?;
        }
        if name == "play" && shell.game_captured {
            key(editor, shell, KEY_ESCAPE, true);
        }
        Ok(())
    }
}

/// 1-SSIM between the Game frame in two captures, each cropped to its `game-frame`
/// rectangle (logical, times `scale` for the capture's pixels) at their common size.
fn game_frames_differ(a: (&Path, Box2), b: (&Path, Box2), scale: f32) -> ara_ui::Result<f64> {
    let size = [
        (a.1.w.min(b.1.w) * scale).floor() as u32,
        (a.1.h.min(b.1.h) * scale).floor() as u32,
    ];
    let crop = |(path, rect): (&Path, Box2)| -> ara_ui::Result<image::RgbaImage> {
        let image = image::open(path)?.into_rgba8();
        Ok(image::imageops::crop_imm(
            &image,
            (rect.x * scale).round() as u32,
            (rect.y * scale).round() as u32,
            size[0],
            size[1],
        )
        .to_image())
    };
    let comparison = ara_render::capture::compare(&crop(a)?, &crop(b)?, 1.)?;
    Ok(comparison.score)
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
    (
        "stop-starting",
        "Stop cancels a Play that is still starting; Play comes back",
    ),
    ("stop-window", "Play in a player window, then Stop ends it"),
    (
        "game-preview",
        "the Game tab before Play shows the scene through its gameplay camera",
    ),
    (
        "play-start",
        "Play's first frames in the Game view match the camera preview",
    ),
    ("play", "Play inside the Game view, walking with W"),
    ("hot-reload", "the edited script hot-reloads while playing"),
    (
        "failure",
        "a broken script: the compile error in Problems with its why",
    ),
    ("stop", "the script fixed, Play stopped"),
    (
        "game-float",
        "the Game tab in its own window shows the gameplay camera preview",
    ),
    (
        "play-float",
        "Play embeds in the floating Game window, sized to it",
    ),
    ("game-dock", "Stop, and the Game tab docks back"),
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
