//! SDL Pilot authoring and truthful retained viewport diagnostic acceptance.
mod fixture;
mod input;
mod observe;
mod pointer;
mod verify;
use super::{input_verify::native, viewport::SceneView};
use crate::world::EditWorldView;
use crate::{Editor, gpu::NativeUi, shell::Shell, state::Camera};
use ara_platform::{Event, Window};
use ara_ui_text::Clipboard;
use glam::{Mat3, Mat4, Quat, Vec3};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const PARENT: &str = "e_nativepilotparent";
const CAMERA: &str = "e_nativepilotcamera";
const BOX: &str = "e_nativepilotbox";
const SPHERE: &str = "e_nativepilotsphere";
const COOKED: &str = "e_nativepilotcooked";
const IDS: [&str; 5] = [PARENT, CAMERA, BOX, SPHERE, COOKED];
const HOLD: u32 = 30;
const PATIENCE: Duration = Duration::from_secs(120);
const CASES: [&str; 8] = [
    "pilot-parent-inverse-native-move-undo-redo",
    "pilot-native-lens-one-undo-redo",
    "pilot-native-escape-no-ops",
    "native-unpilot-badge",
    "agent-presence-does-not-retarget-human-pilot",
    "unowned-human-view-stops-pilot",
    "agent-deleted-target-stops-without-clobber",
    "native-bounds-colliders-observed-stats",
];

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        self.0 = text.into();
        Ok(())
    }
}
pub(super) struct PilotCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    prepared: bool,
    started: Instant,
    doc: String,
    agent: String,
    clipboard: Clip,
    pointer: Option<[f32; 2]>,
    before: Value,
    committed: Value,
    baseline: Camera,
    moved: Camera,
    revision: u64,
    held: u32,
    holding: bool,
    grip: [f32; 2],
    events: usize,
    case_events: usize,
    pending_capture: Option<&'static str>,
    complete: Option<Value>,
    error: Option<String>,
    results: Vec<Value>,
    trace: Vec<Value>,
    fixture: Value,
    cache: Option<(u64, u64)>,
    stable: u32,
    stats_started: Option<Instant>,
    stats_before: u64,
    gesture: Value,
}
impl PilotCheck {
    pub fn new(editor: &mut Editor, shell: &mut Shell, output: &Path) -> ara_ui::Result<Self> {
        if !shell.offscreen || !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err(
                "Native Pilot proof requires the isolated offscreen behaviour fixture".into(),
            );
        }
        if shell.modal_active() || editor.gesture.is_some() || editor.play_world.is_some() {
            return Err(
                "Native Pilot proof requires no modal, authoring gesture or Play world".into(),
            );
        }
        std::fs::create_dir_all(output)?;
        let fixture = fixture::create(editor)?;
        let agent = ara_proto::Endpoint::call(
            &mut editor.host,
            "session.begin",
            json!({"client":"native-pilot-proof","author":"agent:native-pilot-proof"}),
        )
        .map_err(|error| error.message)?["session"]
            .as_str()
            .ok_or("Pilot fixture agent session missing")?
            .to_owned();
        Ok(Self {
            output: output.into(),
            index: 0,
            phase: 0,
            prepared: false,
            started: Instant::now(),
            doc: editor.state.doc.clone(),
            agent,
            clipboard: Default::default(),
            pointer: None,
            before: Value::Null,
            committed: Value::Null,
            baseline: editor.state.camera,
            moved: editor.state.camera,
            revision: 0,
            held: 0,
            holding: false,
            grip: [0.; 2],
            events: 0,
            case_events: 0,
            pending_capture: None,
            complete: None,
            error: None,
            results: Vec::new(),
            trace: Vec::new(),
            fixture,
            cache: None,
            stable: 0,
            stats_started: None,
            stats_before: 0,
            gesture: Value::Null,
        })
    }
    pub fn finished(&self) -> bool {
        self.error.is_some() || self.index == CASES.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|v| v["ok"] != true)
            .filter_map(|v| v["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn clipboard(&mut self) -> &mut dyn Clipboard {
        &mut self.clipboard
    }
    /// Fixed input integration duration; never a claimed GPU timing measurement.
    pub fn fly_dt(&self) -> Option<f32> {
        self.holding.then_some(1. / 120.)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "independent native owners and presented Scene readiness"
    )]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        scene: &SceneView,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if let Some(point) = self.pointer {
            shell.desktop_pointer = Some(point);
        }
        let count = events.len();
        let result = self.input(editor, shell, window, scale, scene, events, scene_ready);
        self.events += events.len() - count;
        self.case_events += events.len() - count;
        if let Err(error) = &result {
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn next(&mut self) {
        self.phase += 1;
        self.started = Instant::now();
    }
    fn done(&mut self, value: Value) {
        self.complete = Some(value);
    }
    fn fail(&mut self, editor: &Editor, shell: &Shell, error: &str) -> ara_ui::Result<()> {
        self.error = Some(error.into());
        self.holding = false;
        self.results.push(json!({"journey":CASES.get(self.index).copied().unwrap_or("evidence-write"),"ok":false,"phase":self.phase,
            "error":error,"nativeEvents":self.case_events,"observed":self.snapshot(editor,shell)}));
        self.write()
    }
    pub fn validate(&self) -> ara_ui::Result<()> {
        if self.index != CASES.len() || self.error.is_some() || !self.failures().is_empty() {
            return Err(format!(
                "Native Pilot/diagnostics proof incomplete: {:?}; {:?}",
                self.failures(),
                self.error
            )
            .into());
        }
        Ok(())
    }
}
