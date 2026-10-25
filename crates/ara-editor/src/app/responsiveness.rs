//! `--verify-responsiveness`: what UI interaction costs and whether every click lands,
//! through the real window loop. Synthetic platform events (pointer moves, button
//! presses at element centres) join the frame's SDL batch, so `route_event`, the
//! controller, dispatch, rebuilds, layout, paint, the UI GPU submit and present all
//! run as they do for a person. Each click is checked by the state it toggles, with
//! normal, same-batch and slow-frame timings. Offscreen only; writes
//! `responsiveness.json` (per-scenario frame costs, UI work counters, GPU UI time,
//! input-to-present latency and click results). The Play scenario clicks Play and
//! measures to the first presented frame showing an embedded Game image (cold, then
//! warm), then clicks Stop. A Scene orbit moves the editor camera every frame.
//!
//! Startup is timed from [`LAUNCHED`]: the first presented frame, when imports
//! settled, and the first Scene frame after that (the full first scene; texture
//! streaming is not tracked). With `--settle-imports before|after` the run waits for
//! imports (capped at [`SETTLE_TIMEOUT`]) before or after the interactions; every
//! frame records whether imports were pending.
use super::*;
use crate::shell::commands::Target;

/// How a click's press and release reach the loop.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
enum Timing {
    /// Press in one frame, release in the next.
    Normal,
    /// Press and release in the same event batch.
    SameBatch,
    /// A 120 ms frame, then press and release in one batch (input queued meanwhile).
    SlowBefore,
    /// Press, a 120 ms frame, then release.
    SlowBetween,
}
const TIMINGS: [Timing; 4] = [
    Timing::Normal,
    Timing::SameBatch,
    Timing::SlowBefore,
    Timing::SlowBetween,
];
const SLOW: Duration = Duration::from_millis(120);
/// Clicks per target and timing.
const CLICKS: usize = 8;

/// What a click targets; each toggles a state the probe can read.
#[derive(Clone, Copy, Debug, serde::Serialize)]
enum Control {
    /// The Scene toolbar's World/Local button (`editor.state.local`).
    Toolbar,
    /// Alternate between the first two tabs of a tabbed dock area.
    Tab,
    /// The File menu title (opens, then closes its menu).
    Menu,
}

enum Act {
    Idle(&'static str, u32),
    Hover(u32),
    Click(Control, Timing),
    /// The View > Theme rows' command, then frames for it to settle.
    Theme(&'static str),
    /// Click Play, wait for the first presented Game frame, click Stop.
    Play,
    /// Move an entity through a typed command (an Inspector edit), then settle.
    Edit,
    /// Type into an Inspector field of a selected mesh one character per frame, then
    /// press Enter (the commit).
    Type(Typed),
    /// Drag a selected mesh with the move gizmo's gesture for `DRAG_FRAMES` frames,
    /// then release (the commit).
    Drag,
    /// Orbit the Scene camera a step every frame for this many frames.
    Orbit(u32),
    /// Wait until imports settle and the Scene shows them (`SETTLE_TIMEOUT`).
    Settle,
}
/// Frames the Scene orbit lasts, and its step per frame (radians).
const ORBIT_FRAMES: u32 = 240;
const ORBIT_STEP: f32 = 0.02;
/// The longest a Settle act waits for imports.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Frames a Drag act moves the gesture before releasing it.
const DRAG_FRAMES: u32 = 12;
/// What a Type act types, and where.
#[derive(Clone, Copy)]
struct Typed {
    /// Scenario name: `type-{name}-{stage}`.
    name: &'static str,
    /// The field's element ID.
    field: &'static str,
    /// The text, one character per frame; every other act types `alternate`, so each
    /// commit changes the value.
    text: &'static str,
    alternate: &'static str,
}
impl Typed {
    fn text(&self, index: usize) -> &'static str {
        if index.is_multiple_of(2) {
            self.text
        } else {
            self.alternate
        }
    }
}
/// A colour (a material edit) and a position (a transform edit, which patches).
const TYPED: [Typed; 2] = [
    Typed {
        name: "color",
        field: "edit-MeshRenderer-color",
        text: "#3366aa",
        alternate: "#aa6633",
    },
    Typed {
        name: "position",
        field: "edit-Transform-position-0",
        text: "2.5",
        alternate: "-1.5",
    },
];

/// A Play attempt in flight.
struct PlayRun {
    clicked: Instant,
    /// When Stop was clicked (`None`: still waiting for the first Game frame).
    stopping: Option<Instant>,
    first_frame_ms: Option<f64>,
    stop_clicked: bool,
}
/// How long a Play attempt may take to show its first frame or to stop.
const PLAY_TIMEOUT: Duration = Duration::from_secs(90);

/// A click in flight.
struct Pending {
    control: Control,
    timing: Timing,
    before: String,
    /// Frames since the press.
    age: u32,
    injected: Instant,
    released: bool,
}

/// One frame's measurements, filled in by the loop.
#[derive(Default, serde::Serialize)]
pub(super) struct Sample {
    pub cpu_ms: f64,
    /// Running the injected command (theme rows), before routing.
    pub inject_ms: f64,
    pub input_ms: f64,
    pub ui_ms: f64,
    /// The authority's tick (server pump, `Editor::tick`).
    pub tick_ms: f64,
    /// Scene updates after document changes, and typed or gizmo previews.
    pub geometry_ms: f64,
    /// Of `geometry_ms`, for a document change: `Editor::view` (resolve), extraction
    /// and upload. A patch (`patched`) extracts and uploads only what moved, timed
    /// together as upload.
    pub view_ms: f64,
    pub extract_ms: f64,
    pub upload_ms: f64,
    pub patched: bool,
    /// A native renderer took the document change (Scene invalidated).
    pub native: bool,
    /// The Scene view's render and overlays (when the frame is dirty).
    pub scene_ms: f64,
    /// Re-rasterizing toolbar icons (theme or DPI changes).
    pub icons_ms: f64,
    pub render_ms: f64,
    pub gpu_ms: f64,
    pub present_ms: f64,
    pub presented: bool,
    /// A new embedded Game frame was shown (`GameView::update`).
    pub game_frame: bool,
    pub rebuilds: u32,
    pub rebuild_ms: f64,
    pub mount_ms: f64,
    pub reconcile_ms: f64,
    /// Of `mount_ms`: the mount's layout frame, and the virtual row pools.
    pub frame_ms: f64,
    pub rows_ms: f64,
    pub work: ara_ui::Work,
    /// Injected input to the end of present, when this frame carried input.
    pub input_to_present_ms: Option<f64>,
    /// The renderer holds a completed Scene frame (`Renderer::presentation_ready`).
    pub presentation_ready: bool,
    /// Imports were pending this frame (or the pipeline had not reported yet).
    pub importing: bool,
}

/// Startup timings in milliseconds from [`LAUNCHED`].
#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Startup {
    /// The first presented frame (the shell, with import progress while loading).
    first_present_ms: Option<f64>,
    /// The first frame with no import pending.
    imports_settled_ms: Option<f64>,
    /// The first Scene frame rendered and presented after imports settled.
    scene_complete_ms: Option<f64>,
}

pub(super) struct Probe {
    output: PathBuf,
    acts: Vec<Act>,
    index: usize,
    age: u32,
    pending: Option<Pending>,
    /// Input injected this frame, for input-to-present.
    injected: Option<Instant>,
    sleep_after: bool,
    hover_targets: Vec<String>,
    samples: Vec<(String, Sample)>,
    clicks: Vec<Value>,
    play: Option<PlayRun>,
    plays: Vec<Value>,
    startup: Startup,
    /// When the current Settle act began.
    settle_began: Option<Instant>,
    /// Whether a Settle act gave up at `SETTLE_TIMEOUT`.
    settle_timed_out: bool,
}

impl Probe {
    pub fn new(output: &std::path::Path, settle: Option<Settle>) -> Self {
        let mut acts = vec![Act::Idle("warmup", 60)];
        if settle == Some(Settle::Before) {
            acts.push(Act::Settle);
        }
        acts.extend([Act::Idle("idle", 240), Act::Hover(160)]);
        for control in [Control::Toolbar, Control::Tab, Control::Menu] {
            for timing in TIMINGS {
                for _ in 0..CLICKS {
                    acts.push(Act::Click(control, timing));
                }
            }
        }
        for _ in 0..6 {
            acts.push(Act::Theme("view.theme.light"));
            acts.push(Act::Theme("view.theme.dark"));
        }
        for _ in 0..6 {
            acts.push(Act::Edit);
        }
        for typed in TYPED {
            for _ in 0..3 {
                acts.push(Act::Type(typed));
            }
        }
        for _ in 0..3 {
            acts.push(Act::Drag);
        }
        acts.push(Act::Idle("idle-after", 120));
        acts.push(Act::Orbit(ORBIT_FRAMES));
        for _ in 0..3 {
            acts.push(Act::Play);
            acts.push(Act::Idle("after-play", 60));
        }
        if settle == Some(Settle::After) {
            acts.push(Act::Settle);
        }
        Self {
            output: output.into(),
            acts,
            index: 0,
            age: 0,
            pending: None,
            injected: None,
            sleep_after: false,
            hover_targets: Vec::new(),
            samples: Vec::new(),
            clicks: Vec::new(),
            play: None,
            plays: Vec::new(),
            startup: Startup::default(),
            settle_began: None,
            settle_timed_out: false,
        }
    }

    pub fn finished(&self) -> bool {
        self.index >= self.acts.len()
    }

    fn label(&self) -> String {
        match self.acts.get(self.index) {
            Some(Act::Idle(name, _)) => (*name).into(),
            Some(Act::Hover(_)) => "hover".into(),
            Some(Act::Click(control, _)) => format!("click-{control:?}").to_lowercase(),
            Some(Act::Theme(_)) => "theme".into(),
            Some(Act::Play) => "play".into(),
            Some(Act::Edit) => "edit".into(),
            Some(Act::Orbit(_)) => "scene-orbit".into(),
            Some(Act::Settle) => "settle".into(),
            Some(Act::Drag) => match self.age {
                0 | 1 => "drag-begin".into(),
                n if n < 2 + DRAG_FRAMES => "drag-move".into(),
                n if n == 2 + DRAG_FRAMES => "drag-commit".into(),
                _ => "drag-settle".into(),
            },
            Some(Act::Type(typed)) => {
                let stage = match self.age {
                    n if n <= 3 => "focus",
                    n if n < 4 + typed.text(self.index).len() as u32 => "keystroke",
                    n if n == 4 + typed.text(self.index).len() as u32 => "commit",
                    _ => "settle",
                };
                format!("type-{}-{stage}", typed.name)
            }
            None => "done".into(),
        }
    }

    /// Before polling: apply a slow frame and reset the shell's counters.
    pub fn begin(&mut self, shell: &mut Shell) {
        let _ = shell.take_stats();
        if let Some(Act::Click(_, Timing::SlowBefore)) = self.acts.get(self.index)
            && self.pending.is_none()
        {
            std::thread::sleep(SLOW);
        }
    }

    /// Whether this frame's `inject` acted (injected events, edits or commands): the
    /// loop counts it as user input.
    pub fn acted(&self) -> bool {
        self.injected.is_some()
    }
    /// Add this frame's synthetic input to the polled batch. `scale` maps window
    /// points to logical pixels (`native::points_to_logical`).
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let at = |point: [f32; 2]| [point[0] / scale, point[1] / scale];
        let push = |events: &mut Vec<(u32, Event)>, event| events.push((0, event));
        self.injected = None;
        if let Some(pending) = &mut self.pending {
            if !pending.released && pending.age >= 1 {
                push(events, button(false));
                pending.released = true;
                self.injected = Some(Instant::now());
            }
            pending.age += 1;
            return Ok(());
        }
        match self.acts.get(self.index) {
            Some(Act::Hover(_)) => {
                if self.hover_targets.is_empty() {
                    self.hover_targets = shell
                        .ui
                        .nodes
                        .iter()
                        .filter(|n| {
                            n.element.tag == "button"
                                && (n.element.id.starts_with("menu-title-")
                                    || n.element.id.starts_with("tool-")
                                    || n.element.id.starts_with("tab-"))
                        })
                        .map(|n| n.element.id.clone())
                        .collect();
                }
                let id = &self.hover_targets[self.age as usize % self.hover_targets.len().max(1)];
                if let Some(point) = centre(shell, id) {
                    push(events, motion(at(point)));
                    self.injected = Some(Instant::now());
                }
            }
            Some(Act::Click(control, timing)) => {
                let (control, timing) = (*control, *timing);
                // A control can be between layouts for a frame (a rebuild in flight):
                // try again next frame, and fail only if it never shows.
                let Some(point) = target(shell, control).and_then(|id| centre(shell, &id)) else {
                    if self.age > 30 {
                        return Err(format!(
                            "responsiveness: no {control:?} control on screen ({:?})",
                            target(shell, control)
                        )
                        .into());
                    }
                    return Ok(());
                };
                push(events, motion(at(point)));
                push(events, button(true));
                let same = matches!(timing, Timing::SameBatch | Timing::SlowBefore);
                if same {
                    push(events, button(false));
                }
                self.sleep_after = timing == Timing::SlowBetween;
                self.injected = Some(Instant::now());
                self.pending = Some(Pending {
                    control,
                    timing,
                    before: signature(editor, shell, control),
                    age: 0,
                    injected: Instant::now(),
                    released: same,
                });
            }
            Some(Act::Play) => {
                let target = match &mut self.play {
                    None => Some("play"),
                    Some(run) if run.stopping.is_some() && !run.stop_clicked => {
                        run.stop_clicked = true;
                        Some("stop")
                    }
                    _ => None,
                };
                if let Some(id) = target {
                    let point = centre(shell, id)
                        .ok_or_else(|| format!("responsiveness: no `{id}` button"))?;
                    push(events, motion(at(point)));
                    push(events, button(true));
                    push(events, button(false));
                    self.injected = Some(Instant::now());
                    if self.play.is_none() {
                        self.play = Some(PlayRun {
                            clicked: Instant::now(),
                            stopping: None,
                            first_frame_ms: None,
                            stop_clicked: false,
                        });
                    }
                }
            }
            Some(Act::Type(typed)) => {
                let (field, text) = (typed.field, typed.text(self.index));
                let typed = text.len() as u32;
                match self.age {
                    0 => {
                        let entity = mesh_entity(editor)?;
                        editor.action("editor.select", json!({"entities": [entity]}))?;
                    }
                    2 => {
                        let point = centre(shell, field).ok_or_else(|| {
                            format!("responsiveness: no `{field}` in the Inspector")
                        })?;
                        push(events, motion(at(point)));
                        push(events, button(true));
                        push(events, button(false));
                    }
                    3 => shell.event(
                        InputEvent::Edit(ara_ui_text::EditEvent::SelectAll),
                        &mut OsClipboard,
                    )?,
                    n if (4..4 + typed).contains(&n) => {
                        let c = &text[(n - 4) as usize..(n - 3) as usize];
                        self.injected = Some(Instant::now());
                        shell.event(
                            InputEvent::Edit(ara_ui_text::EditEvent::Insert(c.into())),
                            &mut OsClipboard,
                        )?;
                    }
                    n if n == 4 + typed => {
                        self.injected = Some(Instant::now());
                        push(events, key(40, true));
                        push(events, key(40, false));
                    }
                    _ => {}
                }
            }
            Some(Act::Drag) => match self.age {
                0 => {
                    let entity = mesh_entity(editor)?;
                    editor.action("editor.select", json!({"entities": [entity]}))?;
                }
                1 => editor.begin_gesture()?,
                n if n < 2 + DRAG_FRAMES => {
                    self.injected = Some(Instant::now());
                    let sign = if self.index.is_multiple_of(2) {
                        1.
                    } else {
                        -1.
                    };
                    if let Some(gesture) = &mut editor.gesture {
                        gesture.update(glam::Vec3::new(sign * 0.05 * (n - 1) as f32, 0., 0.));
                    }
                }
                n if n == 2 + DRAG_FRAMES => {
                    self.injected = Some(Instant::now());
                    editor.finish_gesture(false)?;
                }
                _ => {}
            },
            Some(Act::Edit) if self.age == 0 => {
                let entity = mesh_entity(editor)?;
                self.injected = Some(Instant::now());
                let y = if self.index.is_multiple_of(2) {
                    0.25
                } else {
                    0.0
                };
                editor.apply(
                    vec![json!({"cmd": "property.set", "args": {"entity": entity, "path": "Transform.position", "value": [0.0, y, 0.0]}})],
                    "Probe edit",
                    None,
                )?;
            }
            Some(Act::Orbit(_)) => {
                let mut camera = editor.state.camera;
                camera.yaw += ORBIT_STEP;
                self.injected = Some(Instant::now());
                set_camera(editor, &camera)?;
            }
            Some(Act::Theme(command)) if self.age == 0 => {
                self.injected = Some(Instant::now());
                shell.run_command(editor, command, &Target::None)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// After the frame (present included): record it and advance the script.
    pub fn end(&mut self, editor: &Editor, shell: &mut Shell, mut sample: Sample) {
        let stats = shell.take_stats();
        sample.rebuilds = stats.rebuilds;
        sample.rebuild_ms = stats.rebuild_ms;
        sample.mount_ms = stats.mount_ms;
        sample.reconcile_ms = stats.reconcile_ms;
        sample.frame_ms = stats.frame_ms;
        sample.rows_ms = stats.rows_ms;
        sample.work = stats.work;
        if sample.presented {
            sample.input_to_present_ms = self.injected.map(|t| t.elapsed().as_secs_f64() * 1000.);
        }
        sample.importing = shell
            .import_counts()
            .is_none_or(|(pending, _, _)| pending > 0);
        self.startup_frame(&sample);
        let game_frame = sample.game_frame;
        self.samples.push((self.label(), sample));
        if std::mem::take(&mut self.sleep_after) {
            std::thread::sleep(SLOW);
        }
        if let Some(run) = &mut self.play {
            if run.stopping.is_none() && (game_frame || run.clicked.elapsed() > PLAY_TIMEOUT) {
                if game_frame {
                    run.first_frame_ms = Some(run.clicked.elapsed().as_secs_f64() * 1000.);
                }
                run.stopping = Some(Instant::now());
            } else if let Some(stopping) = run.stopping
                && run.stop_clicked
                && (editor.state.playing.is_none() || stopping.elapsed() > PLAY_TIMEOUT)
            {
                self.plays.push(json!({
                    "attempt": self.plays.len() + 1,
                    "clickToFirstGameFrameMs": run.first_frame_ms,
                    "stopMs": editor.state.playing.is_none()
                        .then(|| stopping.elapsed().as_secs_f64() * 1000.),
                }));
                self.play = None;
                self.advance();
            }
            return;
        }
        if let Some(pending) = &self.pending {
            let now = signature(editor, shell, pending.control);
            let landed = pending.released && now != pending.before;
            if landed || pending.age > 10 {
                self.clicks.push(json!({
                    "control": pending.control,
                    "timing": pending.timing,
                    "ok": landed,
                    "frames": pending.age + 1,
                    "toEffectMs": pending.injected.elapsed().as_secs_f64() * 1000.,
                    "before": pending.before,
                    "after": now,
                }));
                self.pending = None;
                self.advance();
            }
            return;
        }
        self.age += 1;
        let done = match self.acts.get(self.index) {
            Some(Act::Idle(_, frames) | Act::Hover(frames)) => self.age >= *frames,
            Some(Act::Theme(_) | Act::Edit) => self.age >= 8,
            Some(Act::Type(typed)) => self.age >= 12 + typed.text(self.index).len() as u32,
            Some(Act::Drag) => self.age >= 10 + DRAG_FRAMES,
            Some(Act::Orbit(frames)) => self.age >= *frames,
            Some(Act::Settle) => {
                let began = *self.settle_began.get_or_insert_with(Instant::now);
                self.settle_timed_out = began.elapsed() > SETTLE_TIMEOUT;
                self.startup.scene_complete_ms.is_some() || self.settle_timed_out
            }
            // A click's target was not on screen yet (`inject` retries, then fails).
            Some(Act::Click(..) | Act::Play) => false,
            None => true,
        };
        if done {
            self.advance();
        }
    }

    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
    }

    /// Startup milestones, from this frame's sample.
    fn startup_frame(&mut self, sample: &Sample) {
        let Some(now) = LAUNCHED.get().map(|t| t.elapsed().as_secs_f64() * 1000.) else {
            return;
        };
        let startup = &mut self.startup;
        if sample.presented && startup.first_present_ms.is_none() {
            startup.first_present_ms = Some(now);
        }
        if startup.imports_settled_ms.is_none() {
            if !sample.importing {
                startup.imports_settled_ms = Some(now);
            }
        } else if startup.scene_complete_ms.is_none()
            && sample.presented
            && sample.presentation_ready
            && sample.scene_ms > 0.
        {
            startup.scene_complete_ms = Some(now);
        }
    }

    /// Write `responsiveness.json`: per-scenario statistics, every click and frame.
    pub fn write(&self, device: &Value) -> ara_ui::Result<()> {
        let mut scenarios = serde_json::Map::new();
        let mut names: Vec<&str> = self.samples.iter().map(|(n, _)| n.as_str()).collect();
        names.dedup();
        for name in names {
            let frames: Vec<&Sample> = self
                .samples
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, s)| s)
                .collect();
            let stat = |f: &dyn Fn(&Sample) -> f64| {
                statistics(&frames.iter().map(|s| f(s)).collect::<Vec<_>>())
            };
            let latencies: Vec<f64> = frames
                .iter()
                .filter_map(|s| s.input_to_present_ms)
                .collect();
            scenarios.insert(
                name.into(),
                json!({
                    "frames": frames.len(),
                    "presented": frames.iter().filter(|s| s.presented).count(),
                    "cpuMs": stat(&|s| s.cpu_ms),
                    "inputMs": stat(&|s| s.input_ms),
                    "uiFrameMs": stat(&|s| s.ui_ms),
                    "rebuildMs": stat(&|s| s.rebuild_ms),
                    "mountMs": stat(&|s| s.mount_ms),
                    "reconcileMs": stat(&|s| s.reconcile_ms),
                    "sceneMs": stat(&|s| s.scene_ms),
                    "tickMs": stat(&|s| s.tick_ms),
                    "geometryMs": stat(&|s| s.geometry_ms),
                    "rebuilds": frames.iter().map(|s| u64::from(s.rebuilds)).sum::<u64>(),
                    "renderMs": stat(&|s| s.render_ms),
                    "gpuWaitMs": stat(&|s| s.gpu_ms),
                    "presentMs": stat(&|s| s.present_ms),
                    "restyled": stat(&|s| s.work.restyled as f64),
                    "layoutPasses": stat(&|s| s.work.layout_passes as f64),
                    "shaped": stat(&|s| s.work.shaped as f64),
                    "paintNodes": stat(&|s| s.work.paint_nodes as f64),
                    "inputToPresentMs": (!latencies.is_empty()).then(|| statistics(&latencies)),
                }),
            );
        }
        let landed = self.clicks.iter().filter(|c| c["ok"] == true).count();
        let report = json!({
            "clicks": {"total": self.clicks.len(), "landed": landed, "each": self.clicks},
            "play": self.plays,
            "startup": self.startup,
            "settleTimedOut": self.settle_timed_out,
            "scenarios": scenarios,
            "frames": self.samples.iter().map(|(name, s)| json!({"scenario": name, "sample": s})).collect::<Vec<_>>(),
            "device": device,
            "note": "Times are wall-clock on the main thread; gpuWaitMs is the time spent waiting for UI submissions (each waits for its fence). Slow-frame clicks add a 120 ms sleep.",
        });
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("responsiveness.json"),
            ara_json::to_canonical_string(&report),
        )?;
        Ok(())
    }
}

/// The first entity with a Transform and a MeshRenderer (what Type and Drag edit).
fn mesh_entity(editor: &Editor) -> ara_ui::Result<String> {
    Ok(
        crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?
            .into_iter()
            .find(|(_, e)| {
                e.record.contains_key("Transform") && e.record.contains_key("MeshRenderer")
            })
            .map(|(id, _)| id)
            .ok_or("responsiveness: no mesh entity to edit")?,
    )
}
fn motion(position: [f32; 2]) -> Event {
    Event::MouseMotion {
        position,
        delta: [0., 0.],
    }
}
fn key(scancode: i32, pressed: bool) -> Event {
    Event::Key {
        scancode,
        pressed,
        repeat: false,
    }
}
fn button(pressed: bool) -> Event {
    Event::MouseButton { button: 1, pressed }
}
fn centre(shell: &Shell, id: &str) -> Option<[f32; 2]> {
    let r = shell.ui.nodes[shell.ui.find(id)?].rect;
    (r.w > 0. && r.h > 0.).then_some([r.x + r.w * 0.5, r.y + r.h * 0.5])
}
/// The element a click on `control` presses now (for tabs: the inactive one of the
/// first tabbed area's first two tabs).
fn target(shell: &Shell, control: Control) -> Option<String> {
    match control {
        Control::Toolbar => Some("tool-space".into()),
        Control::Menu => Some("menu-title-0".into()),
        Control::Tab => {
            let [a, b] = tab_pair(shell)?;
            Some(if selected(shell, &a) { b } else { a })
        }
    }
}
/// The first two tabs of the first dock area showing a tab bar.
fn tab_pair(shell: &Shell) -> Option<[String; 2]> {
    let bar = shell
        .ui
        .nodes
        .iter()
        .find(|n| n.element.id.starts_with("tabs-") && n.visible)?;
    let mut tabs = bar
        .element
        .children
        .iter()
        .map(|i| &shell.ui.nodes[*i])
        .filter(|n| n.element.tag == "button" && n.element.id.starts_with("tab-"))
        .map(|n| n.element.id.clone());
    Some([tabs.next()?, tabs.next()?])
}
fn selected(shell: &Shell, id: &str) -> bool {
    shell.ui.find(id).is_some_and(|i| {
        shell.ui.nodes[i]
            .element
            .attrs
            .get("class")
            .is_some_and(|c| c.split(' ').any(|c| c == "tab-selected"))
    })
}
/// The state a click on `control` changes.
fn signature(editor: &Editor, shell: &Shell, control: Control) -> String {
    match control {
        Control::Toolbar => editor.state.local.to_string(),
        Control::Menu => format!("{:?}", shell.menu.as_ref().and_then(|m| m.bar())),
        Control::Tab => tab_pair(shell)
            .map(|pair| pair.into_iter().filter(|t| selected(shell, t)).collect())
            .unwrap_or_default(),
    }
}
