//! J7 post-volume/HeightFog authoring through raw SDL; probe/render parity remains separate.
use super::input_verify::native;
use crate::{Editor, gpu::NativeUi, shell::Shell};
use ara_platform::{Event, Window};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
mod input;
mod proof;
#[cfg(test)]
mod tests;
const SCENE: &str = "sc_lightingnative";
const PATIENCE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Open,
    Global,
    Box,
    Sphere,
    Fog,
    ShapeSphere,
    ShapeBox,
    Priority,
    Weight,
    Blend,
    Scrub,
    Interior,
    Cage,
    FogDensity,
    FogWinner,
    Walk,
}
const ACTS: &[Act] = &[
    Act::Open,
    Act::Global,
    Act::Box,
    Act::Sphere,
    Act::Fog,
    Act::ShapeSphere,
    Act::ShapeBox,
    Act::Priority,
    Act::Weight,
    Act::Blend,
    Act::Scrub,
    Act::Interior,
    Act::Cage,
    Act::FogDensity,
    Act::FogWinner,
    Act::Walk,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "lighting-native-palette-open",
            Self::Global => "lighting-create-global-one-undo-redo",
            Self::Box => "lighting-create-box-one-undo-redo",
            Self::Sphere => "lighting-create-sphere-one-undo-redo",
            Self::Fog => "lighting-create-height-fog-one-undo-redo",
            Self::ShapeSphere => "lighting-inspector-shared-select-sphere",
            Self::ShapeBox => "lighting-inspector-shared-select-box",
            Self::Priority => "lighting-inspector-priority-numeric",
            Self::Weight => "lighting-inspector-weight-numeric",
            Self::Blend => "lighting-inspector-blend-distance-units",
            Self::Scrub => "lighting-weight-scrub-preview-one-undo",
            Self::Interior => "lighting-interior-preset-atomic-append",
            Self::Cage => "lighting-native-selected-volume-cage",
            Self::FogDensity => "lighting-height-fog-density-schema-control",
            Self::FogWinner => "lighting-second-fog-first-id-winner",
            Self::Walk => "lighting-native-camera-outside-blend-inside",
        }
    }
    fn creation(self) -> bool {
        matches!(
            self,
            Self::Global | Self::Box | Self::Sphere | Self::Fog | Self::FogWinner
        )
    }
    fn mutation(self) -> bool {
        !matches!(self, Self::Open | Self::Cage | Self::Walk)
    }
    fn field(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Priority => Some(("PostProcessVolume-priority", "23")),
            Self::Weight => Some(("PostProcessVolume-weight", "0.4")),
            Self::Blend => Some(("PostProcessVolume-blendDistance", "3.5 m")),
            Self::FogDensity => Some(("HeightFog-density", "0.004")),
            _ => None,
        }
    }
}
pub(super) struct LightingCheck {
    output: PathBuf,
    index: usize,
    phase: u32,
    started: Instant,
    case_started: Instant,
    pending: Option<&'static str>,
    error: Option<String>,
    stopped: bool,
    restored: bool,
    original_doc: String,
    original_camera: crate::state::Camera,
    original_selection: Vec<String>,
    ids: BTreeMap<String, String>,
    target: Option<String>,
    before: Value,
    after: Value,
    observations: Value,
    checks: Value,
    trace: Vec<Value>,
    results: Vec<Value>,
    native_events: usize,
    scrub_start: [f32; 2],
    samples: Vec<Value>,
}
impl LightingCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Lighting proof requires the isolated behaviour fixture".into());
        }
        let original_doc = editor.state.doc.clone();
        let original_camera = editor.state.camera;
        let original_selection = editor.state.selection.clone();
        editor.apply(vec![json!({"cmd":"doc.create","args":{"id":SCENE,"path":"levels/lighting-native.scene","kind":"scene"}})],"Native Lighting fixture document (typed setup)",None)?;
        editor.action("editor.scene.open", json!({"scene":SCENE}))?;
        editor.apply(vec![
            json!({"cmd":"entity.create","args":{"id":"e_lightingproofmesh","name":"Lighting proof box","components":{"Transform":{},"MeshRenderer":{"size":[2,2,2]}}}}),
            json!({"cmd":"entity.create","args":{"id":"e_lightingproofsun","name":"Lighting proof sun","components":{"Transform":{"rotation":[-40,25,0]},"Light":{"kind":"directional","intensity":3}}}}),
        ],"Native Lighting geometry/light fixture (typed setup)",None)?;
        let check = Self {
            output: output.into(),
            index: 0,
            phase: 0,
            started: Instant::now(),
            case_started: Instant::now(),
            pending: None,
            error: None,
            stopped: false,
            restored: false,
            original_doc,
            original_camera,
            original_selection,
            ids: BTreeMap::new(),
            target: None,
            before: Value::Null,
            after: Value::Null,
            observations: json!({}),
            checks: json!({}),
            trace: Vec::new(),
            results: Vec::new(),
            native_events: 0,
            scrub_start: [0.; 2],
            samples: Vec::new(),
        };
        check.write()?;
        Ok(check)
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len() && self.restored && !self.stopped
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|row| row["ok"] != true)
            .filter_map(|row| row["journey"].as_str().map(str::to_owned))
            .collect()
    }
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.stopped || self.finished() {
            return Ok(());
        }
        if self.error.is_some() {
            native::focus_lost(window, events)?;
            return Ok(());
        }
        if self.pending.is_some() {
            return Ok(());
        }
        if self.index >= ACTS.len() {
            native::focus_lost(window, events)?;
            editor.action("editor.scene.open", json!({"scene":self.original_doc}))?;
            editor.action(
                "editor.camera",
                json!({"projection":self.original_camera.projection}),
            )?;
            let camera = self.original_camera;
            editor.action("editor.camera",json!({"target":camera.target.to_array(),"yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance,"fovYDegrees":camera.fov_y_degrees,"orthographicHeight":camera.orthographic_height}))?;
            editor.action("editor.select", json!({"entities":self.original_selection}))?;
            editor.action("editor.layout.reset", json!({}))?;
            shell.rebuild(editor)?;
            self.restored = editor.state.doc == self.original_doc
                && editor.state.camera == self.original_camera;
            return self.write();
        }
        let began = events.len();
        let result = if self.case_started.elapsed() >= PATIENCE {
            Err(format!(
                "Lighting native wait exceeded {} seconds at phase {}",
                PATIENCE.as_secs(),
                self.phase
            )
            .into())
        } else {
            self.step(editor, shell, window, scale, events)
        };
        self.native_events += events.len().saturating_sub(began);
        if let Err(error) = result {
            events.truncate(began);
            native::focus_lost(window, events)?;
            self.error = Some(error.to_string());
        }
        Ok(())
    }
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        scene: &super::viewport::SceneView,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() || self.stopped {
            return Ok(());
        }
        if let Some(error) = self.error.take() {
            self.results.push(json!({"journey":ACTS.get(self.index).map(|act|act.label()),"ok":false,"error":error,"phase":self.phase,"before":self.before,"after":self.after,"checks":self.checks,"observations":self.observations,"trace":self.trace,"viewportDiagnostics":shell.layout_report()["viewportDiagnostics"]}));
            self.stopped = true;
            self.write()?;
            return Err(
                "Native Lighting post/fog proof stopped after a durable failed story".into(),
            );
        }
        let Some(stage) = self.pending else {
            return Ok(());
        };
        if !scene_ready {
            if self.case_started.elapsed() >= PATIENCE {
                self.error = Some(format!(
                    "Lighting capture {stage} did not receive a current completed Scene"
                ));
            }
            return Ok(());
        }
        let act = ACTS[self.index];
        let prefix = format!("lighting-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.select_target(0);
        gpu.pixels()?
            .save(self.output.join(format!("{prefix}.png")))?;
        let layout = shell.layout_report();
        std::fs::write(
            self.output.join(format!("{prefix}.layout.json")),
            ara_json::to_canonical_string(&layout),
        )?;
        self.trace.push(json!({"stage":stage,"phase":self.phase,"content":proof::snapshot(editor,None)?,"viewportDiagnostics":layout["viewportDiagnostics"],"nativeSourceCurrent":scene.diagnostic_source(editor).current}));
        self.pending = None;
        let result = self.observe(stage, editor, shell, scene);
        if let Err(error) = result {
            if self.stopped {
                return Err(error);
            }
            self.error = Some(error.to_string());
        }
        self.write()
    }
    fn finish_case(&mut self) -> ara_ui::Result<()> {
        let ok = self
            .checks
            .as_object()
            .is_some_and(|checks| !checks.is_empty() && checks.values().all(|value| value == true));
        self.results.push(json!({"journey":ACTS[self.index].label(),"ok":ok,"checks":self.checks,"before":self.before,"after":self.after,"observations":self.observations,"trace":self.trace,"nativeEventsTotal":self.native_events}));
        if !ok {
            self.stopped = true;
            self.write()?;
            return Err("Native Lighting semantic assertion failed; report retained".into());
        }
        self.index += 1;
        self.phase = 0;
        self.case_started = Instant::now();
        self.before = Value::Null;
        self.after = Value::Null;
        self.checks = json!({});
        self.observations = json!({});
        self.trace.clear();
        self.samples.clear();
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("lighting.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"restored":self.restored,"stoppedOnFailure":self.stopped,"expected":ACTS.len(),"attempted":self.results.len(),"passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results,"current":{"journey":ACTS.get(self.index).map(|act|act.label()),"phase":self.phase,"capture":self.pending,"seconds":self.case_started.elapsed().as_secs_f64()},"elapsedSeconds":self.started.elapsed().as_secs_f64(),"nativeEvents":self.native_events,
            "scope":{"postAndFogAuthoring":true,"retainedNativeFrameBlend":true,"appliedRendererPostSettingsMeasured":false,"scenePlayPixelParity":false,"reflectionProbeCapture":false,"giOverdrawDebug":false},"remainingJ7":["Reflection probe capture/recapture is not established by this route","Applied Scene/Play renderer parity and GI/overdraw debug require separate measured evidence"],"setup":"Isolated typed doc.create/entity.create and navigation only; measured authoring uses SDL controls and ordinary history shortcuts","snapshots":"Canonical record-map UTF-8 byte length + BLAKE3; source fixture retained on disk"}),
            ),
        )?;
        Ok(())
    }
}
