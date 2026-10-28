//! J2 mesh snap proof: native pointer/key events, transient previews and one typed undo.
use super::*;
use std::path::Path;
mod fixture;
mod input;
mod proof;

const SOURCE: &str = "e_snapmesh01";
const CHILD: &str = "e_snapchild01";
const TARGET: &str = "e_snapramp01";
const EMPTY: &str = "e_snapempty01";
const BEFORE: u32 = 6;
const PREVIEW: u32 = 12;
const AFTER: u32 = 18;
const FINISH: u32 = 30;
const READINESS_PATIENCE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Surface,
    Vertex,
    Escape,
    Invalid,
    Drop,
    DropInvalid,
    Ortho,
    Floating,
    DeletedTarget,
}
const ACTS: &[Act] = &[
    Act::Surface,
    Act::Vertex,
    Act::Escape,
    Act::Invalid,
    Act::Drop,
    Act::DropInvalid,
    Act::Ortho,
    Act::Floating,
    Act::DeletedTarget,
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::Surface => "tilted-surface-offset-grid-bypass",
            Self::Vertex => "source-target-vertex",
            Self::Escape => "snap-escape-no-ops",
            Self::Invalid => "invalid-snap-release-no-ops",
            Self::Drop => "drop-to-actual-surface",
            Self::DropInvalid => "drop-selection-atomic-rejection",
            Self::Ortho => "orthographic-surface-contact",
            Self::Floating => "floating-vertex-contact",
            Self::DeletedTarget => "deleted-target-cancels-snap",
        }
    }
    fn vertex(self) -> bool {
        matches!(self, Self::Vertex | Self::Floating)
    }
    fn drop(self) -> bool {
        matches!(self, Self::Drop | Self::DropInvalid)
    }
    fn commits(self) -> bool {
        matches!(
            self,
            Self::Surface | Self::Vertex | Self::Drop | Self::Ortho | Self::Floating
        )
    }
}

pub(super) struct SnapCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    wait_started: Option<Instant>,
    waiting_captured: bool,
    pending_failure: Option<String>,
    readiness: Value,
    from: [f32; 2],
    to: [f32; 2],
    delta: Vec3,
    source: Option<Vec3>,
    target: Option<Vec3>,
    normal: Option<Vec3>,
    vertices: Vec<Vec3>,
    revision: u64,
    before: Value,
    committed: Option<Value>,
    trace: Vec<Value>,
    results: Vec<Value>,
}
impl SnapCheck {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Snap verification requires the isolated behaviour fixture".into());
        }
        fixture::create(editor)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            wait_started: None,
            waiting_captured: false,
            pending_failure: None,
            readiness: Value::Null,
            from: [0.; 2],
            to: [0.; 2],
            delta: Vec3::ZERO,
            source: None,
            target: None,
            normal: None,
            vertices: Vec::new(),
            revision: 0,
            before: Value::Null,
            committed: None,
            trace: Vec::new(),
            results: Vec::new(),
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["journey"].as_str().map(str::to_owned))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        floats: &windows::Floats,
        scale: f32,
        scene: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if self.age == 0 {
            // Each proof owns its input lifecycle. Cancel any failed preceding
            // gesture through the production route rather than clearing its model.
            events.push((window.id(), Event::FocusLost));
            self.trace.push(json!({"stage":"inherited-input","camera":editor.state.camera,"doc":editor.state.doc,
                "selection":editor.state.selection,"gestureActive":editor.gesture.is_some(),"hidden":editor.state.hidden,"locked":editor.state.locked}));
            return self.setup(act, editor, shell);
        }
        if self.age == BEFORE {
            match self.acquire(act, editor, shell, scene) {
                Ok(true) => {
                    if self
                        .wait_started
                        .is_some_and(|started| started.elapsed() >= READINESS_PATIENCE)
                    {
                        let message = format!(
                            "Native snap fixture became ready after its {} real-second deadline: {}",
                            READINESS_PATIENCE.as_secs(),
                            act.label()
                        );
                        editor.report_error(&message);
                        self.pending_failure = Some(message);
                        self.waiting = true;
                        return Ok(());
                    }
                }
                Ok(false) => {
                    self.waiting = true;
                    self.waited += 1;
                    let started = self.wait_started.get_or_insert_with(Instant::now);
                    if started.elapsed() >= READINESS_PATIENCE {
                        let message = format!(
                            "Native snap fixture did not become ready within {} real seconds: {} ({})",
                            READINESS_PATIENCE.as_secs(),
                            act.label(),
                            self.readiness["reason"]
                                .as_str()
                                .unwrap_or("unknown rejection")
                        );
                        editor.report_error(&message);
                        self.pending_failure = Some(message);
                    }
                    return Ok(());
                }
                Err(error) => {
                    let message =
                        format!("Native snap acquisition failed: {}: {error}", act.label());
                    editor.report_error(&message);
                    self.pending_failure = Some(message);
                    self.waiting = true;
                    return Ok(());
                }
            }
        }
        self.input(act, editor, shell, window, floats, scale, events)
    }

    /// Capture the owning hidden native target after the production frame presents.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        let act = ACTS[self.index];
        if let Some(message) = self.pending_failure.take() {
            let capture_error = self
                .capture(act, "not-ready", editor, shell, gpu, floats)
                .err()
                .map(|error| error.to_string());
            self.results.push(json!({"journey":act.label(),"ok":false,"error":message,
                "readiness":self.readiness,"readinessSample":"before-native-frame","camera":editor.state.camera,"doc":editor.state.doc,
                "selection":editor.state.selection,"gestureActive":editor.gesture.is_some(),"waitedSeconds":self.wait_started.map(|started|started.elapsed().as_secs_f64()),
                "waitedFrames":self.waited,"captureError":capture_error,"trace":self.trace}));
            self.write()?;
            return Err(message.into());
        }
        if self.waiting {
            if !self.waiting_captured {
                if let Err(error) = self.capture(act, "waiting", editor, shell, gpu, floats) {
                    self.trace
                        .push(json!({"stage":"waiting-capture-failed","error":error.to_string()}));
                }
                self.waiting_captured = true;
                self.write()?;
            }
            return Ok(());
        }
        if [BEFORE, PREVIEW, AFTER, 22, 26].contains(&self.age) {
            let stage = match self.age {
                BEFORE => "before",
                PREVIEW => "preview",
                AFTER => "after",
                22 => "undo",
                _ => "redo",
            };
            self.capture(act, stage, editor, shell, gpu, floats)?;
            if self.age == AFTER {
                self.committed = Some(fixture::records(editor)?);
            }
        }
        if self.age >= FINISH {
            let (ok, detail) = self.result(act, editor)?;
            self.results
                .push(json!({"journey":act.label(),"ok":ok,"detail":detail,"trace":self.trace}));
            self.index += 1;
            self.age = 0;
            self.waited = 0;
            self.wait_started = None;
            self.waiting_captured = false;
            self.readiness = Value::Null;
            self.trace.clear();
            self.committed = None;
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("snapping.json"),
            ara_json::to_canonical_string(
                &json!({"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),"journeys":self.results,"geometry":"presented native mesh triangles and primary-LOD vertices; no ground-plane fallback","input":"SDL events tagged to the Scene host with its own display/UI scale",
                    "current":{"journey":ACTS.get(self.index).map(|act|act.label()),"age":self.age,"readiness":self.readiness,"readinessSample":"before-native-frame",
                        "waitedSeconds":self.wait_started.map(|started|started.elapsed().as_secs_f64()),"waitedFrames":self.waited,"limitSeconds":READINESS_PATIENCE.as_secs()}}),
            ),
        )?;
        Ok(())
    }
}
