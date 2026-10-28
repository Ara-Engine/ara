//! `--verify-behaviours`: content gestures through the production native event router.
//! Every capture is read from a hidden window's completed native target.
use super::*;
use ara_ui::paint::Box2;
use std::path::Path;
mod fixture;
mod hierarchy;
mod input;
mod verify;
pub(super) use fixture::fixture;

const IDS: [&str; 5] = [
    "e_draga01",
    "e_dragb01",
    "e_dragc01",
    "e_dragd01",
    "e_drage01",
];
const CAMERA: &str = "e_dragcam1";
const SETTLE: u32 = 6;
const BEGIN: u32 = SETTLE + 1;
const PREVIEW: u32 = BEGIN + 4;
const RELEASE: u32 = BEGIN + 7;
const FINISH: u32 = RELEASE + SETTLE;
const PATIENCE: u32 = 144 * 120;
const SCENE_PATIENCE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug)]
enum Act {
    MultiReorder,
    Reparent,
    Cycle,
    Cancel,
    AssetReference,
    InvalidAsset,
    AssetSurface,
    MaterialSurface,
    EntityReference,
    Component,
    FloatingReference,
    OsImport,
    Hierarchy(hierarchy::Action),
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod readiness_tests {
    use super::*;

    fn at(age: u32) -> ContentDrags {
        ContentDrags {
            output: PathBuf::new(),
            index: 0,
            age,
            waiting: false,
            waited: 0,
            scene_wait: None,
            scene_wait_polls: 0,
            cancel_pending: false,
            failed_frame: false,
            source: None,
            before_revision: 0,
            before_entities: Vec::new(),
            results: Vec::new(),
            trace: Vec::new(),
            hierarchy_proof: Value::Null,
        }
    }

    #[test]
    fn every_capture_and_input_start_waits_for_presented_scene_without_advancing() {
        for age in [SETTLE, BEGIN, PREVIEW, FINISH] {
            let mut check = at(age);
            let now = Instant::now();
            assert!(!check.checkpoint_ready(false, now).unwrap());
            assert!(
                !check
                    .checkpoint_ready(false, now + Duration::from_secs(1))
                    .unwrap()
            );
            assert_eq!(check.age, age);
            assert_eq!(check.trace[0]["sceneReady"], false);
            assert!(
                check
                    .checkpoint_ready(true, now + Duration::from_secs(2))
                    .unwrap()
            );
            assert_eq!(check.trace[1]["stage"], "scene-ready");
            assert_eq!(check.trace[1]["waitSeconds"], 2.);
            assert!(check.scene_wait.is_none());
        }
        assert!(
            at(BEGIN + 1)
                .checkpoint_ready(false, Instant::now())
                .unwrap()
        );
    }

    #[test]
    fn deadline_is_wall_time_and_cannot_accept_late_readiness() {
        let mut check = at(PREVIEW);
        let now = Instant::now();
        // Fast offscreen UI frames cannot consume a time budget prematurely.
        for _ in 0..20_000 {
            assert!(!check.checkpoint_ready(false, now).unwrap());
        }
        assert!(check.checkpoint_ready(true, now + SCENE_PATIENCE).is_err());
        let failure = check.trace.last().unwrap();
        assert_eq!(failure["stage"], "scene-timeout");
        assert_eq!(failure["sceneReady"], true);
        assert_eq!(failure["limitSeconds"], SCENE_PATIENCE.as_secs());
    }
}
const ACTS: &[Act] = &[
    Act::MultiReorder,
    Act::Reparent,
    Act::Cycle,
    Act::Cancel,
    Act::AssetReference,
    Act::InvalidAsset,
    Act::AssetSurface,
    Act::MaterialSurface,
    Act::EntityReference,
    Act::Component,
    Act::FloatingReference,
    Act::OsImport,
    Act::Hierarchy(hierarchy::Action::Before),
    Act::Hierarchy(hierarchy::Action::Inside),
    Act::Hierarchy(hierarchy::Action::After),
    Act::Hierarchy(hierarchy::Action::Root),
];
impl Act {
    fn label(self) -> &'static str {
        match self {
            Self::MultiReorder => "hierarchy-multi-reorder",
            Self::Reparent => "hierarchy-reparent",
            Self::Cycle => "hierarchy-invalid-cycle",
            Self::Cancel => "drag-escape",
            Self::AssetReference => "asset-reference",
            Self::InvalidAsset => "asset-invalid-reference",
            Self::AssetSurface => "asset-surface-placement",
            Self::MaterialSurface => "material-submesh-drop",
            Self::EntityReference => "entity-reference",
            Self::Component => "component-reorder",
            Self::FloatingReference => "floating-reference",
            Self::OsImport => "os-file-import",
            Self::Hierarchy(action) => action.label(),
        }
    }
    fn finish(self) -> u32 {
        FINISH
            + if matches!(self, Self::Hierarchy(_)) {
                6
            } else {
                0
            }
    }
}
#[derive(Clone, Copy)]
struct Point {
    local: [f32; 2],
    desktop: [f32; 2],
    scale: f32,
    sdl: u32,
}

pub(super) struct ContentDrags {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    scene_wait: Option<Instant>,
    scene_wait_polls: u64,
    cancel_pending: bool,
    failed_frame: bool,
    source: Option<Point>,
    before_revision: u64,
    before_entities: Vec<String>,
    results: Vec<Value>,
    trace: Vec<Value>,
    hierarchy_proof: Value,
}
impl ContentDrags {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        if !editor.host.root().join(".ara-behaviour-fixture").is_file() {
            return Err("Behaviour verification requires its isolated fixture project".into());
        }
        editor.apply(
            vec![json!({"cmd":"import.initialize","args":{"source":"assets/rock.glb","preset":"fast"}})],
            "Initialize isolated behaviour model import",
            None,
        )?;
        let mut commands: Vec<_> = IDS.iter().enumerate().map(|(n, id)| json!({"cmd":"entity.create","args":{"id":id,"name":format!("Drag object {}",n+1),"components":{"Transform":{"position":[n as f32 * 2. - 4.,0.5,0.]},"MeshRenderer":{}}}})).collect();
        commands.push(json!({"cmd":"entity.create","args":{"id":CAMERA,"name":"Follow camera","components":{"Transform":{"position":[0.,3.,-6.]},"Camera":{},"FollowCamera":{}}}}));
        commands.push(json!({"cmd":"entity.create","args":{"id":"e_dragfloor","name":"Drop surface","components":{"Transform":{},"MeshRenderer":{"shape":"plane","size":[30.,1.,30.]}}}}));
        editor.apply(commands, "Create isolated behaviour fixtures", None)?;
        editor.action(
            "editor.resource.create",
            json!({"path":"assets/coating.graph","domain":"material"}),
        )?;
        editor.action("editor.preferences", json!({"reduceMotion":true}))?;
        std::fs::create_dir_all(output)?;
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            scene_wait: None,
            scene_wait_polls: 0,
            cancel_pending: false,
            failed_frame: false,
            source: None,
            before_revision: 0,
            before_entities: Vec::new(),
            results: Vec::new(),
            trace: Vec::new(),
            hierarchy_proof: Value::Null,
        })
    }
    pub fn finished(&self) -> bool {
        self.index >= ACTS.len()
    }
    pub fn failures(&self) -> Vec<String> {
        self.results
            .iter()
            .filter(|result| result["ok"] != true)
            .filter_map(|result| result["behaviour"].as_str().map(str::to_owned))
            .collect()
    }
    /// Queue native input before the loop dispatches its event batch.
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
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.cancel_pending) {
            self.cancel_input(shell, events);
            self.failed_frame = true;
            return Ok(());
        }
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        let result = (|| {
            if !self.checkpoint_ready(scene_ready, Instant::now())? {
                self.waiting = true;
                return Ok(());
            }
            self.input(act, editor, shell, window, floats, scale, scene, events)
        })();
        if self.waiting && self.scene_wait_polls == 1 {
            self.write()?;
        }
        if let Err(error) = result {
            self.cancel_input(shell, events);
            self.results.push(json!({"behaviour":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
            self.advance();
            self.failed_frame = true;
            self.write()?;
        }
        Ok(())
    }
    fn cancel_input(&self, shell: &mut Shell, events: &mut Vec<(u32, Event)>) {
        shell.leave();
        shell.desktop_pointer = None;
        events.retain(|(_, event)| !matches!(event, Event::MouseButton { .. }));
        for pressed in [true, false] {
            events.push((
                0,
                Event::Key {
                    scancode: 41,
                    pressed,
                    repeat: false,
                },
            ));
        }
    }
    /// Native UI frames can complete before their asynchronous Scene image. Hold
    /// the exact checkpoint, using wall time rather than a guessed frame count.
    fn checkpoint_ready(&mut self, ready: bool, now: Instant) -> ara_ui::Result<bool> {
        if ![SETTLE, BEGIN, PREVIEW, FINISH, FINISH + 3, FINISH + 6].contains(&self.age) {
            return Ok(true);
        }
        if let Some(started) = self.scene_wait
            && now.duration_since(started) >= SCENE_PATIENCE
        {
            self.trace.push(json!({"stage":"scene-timeout","age":self.age,"sceneReady":ready,
                "waitSeconds":now.duration_since(started).as_secs_f64(),"polls":self.scene_wait_polls,
                "limitSeconds":SCENE_PATIENCE.as_secs()}));
            return Err(format!(
                "Presented Scene did not become ready at checkpoint {} within {} seconds",
                self.age,
                SCENE_PATIENCE.as_secs()
            )
            .into());
        }
        if ready {
            if let Some(started) = self.scene_wait.take() {
                self.trace.push(json!({"stage":"scene-ready","age":self.age,"sceneReady":true,
                    "waitSeconds":now.duration_since(started).as_secs_f64(),"polls":self.scene_wait_polls}));
            }
            self.scene_wait_polls = 0;
            return Ok(true);
        }
        if self.scene_wait.is_none() {
            self.scene_wait = Some(now);
            self.trace.push(
                json!({"stage":"scene-wait","age":self.age,"sceneReady":false,
                "limitSeconds":SCENE_PATIENCE.as_secs()}),
            );
        }
        self.scene_wait_polls += 1;
        Ok(false)
    }
    fn advance(&mut self) {
        self.index += 1;
        self.age = 0;
        self.waited = 0;
        self.scene_wait = None;
        self.scene_wait_polls = 0;
        self.source = None;
        self.trace.clear();
        self.hierarchy_proof = Value::Null;
    }
    /// Capture an already-composited frame and verify the mutation after the gesture settles.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if std::mem::take(&mut self.failed_frame) || self.finished() {
            return Ok(());
        }
        if self.waiting {
            if self.scene_wait.is_none() {
                self.waited += 1;
            }
            return Ok(());
        }
        let act = ACTS[self.index];
        match self.checkpoint_ready(scene_ready, Instant::now()) {
            Ok(true) => {}
            Ok(false) => {
                if self.scene_wait_polls == 1 {
                    self.write()?;
                }
                return Ok(());
            }
            Err(error) => {
                self.results.push(json!({"behaviour":act.label(),"ok":false,"error":error.to_string(),"trace":self.trace}));
                shell.desktop_pointer = None;
                self.cancel_pending = true;
                self.advance();
                return self.write();
            }
        }
        let history_stage =
            matches!(act, Act::Hierarchy(_)) && [FINISH + 3, FINISH + 6].contains(&self.age);
        if [SETTLE, PREVIEW, FINISH].contains(&self.age) || history_stage {
            let stage = if self.age == SETTLE {
                "before"
            } else if self.age == PREVIEW {
                "preview"
            } else if self.age == FINISH + 3 {
                "undo"
            } else if self.age == FINISH + 6 {
                "redo"
            } else {
                "after"
            };
            let prefix = format!("{:02}-{}-{stage}", self.index + 1, act.label());
            gpu.select_target(0);
            gpu.pixels()?
                .save(self.output.join(format!("{prefix}.png")))?;
            std::fs::write(
                self.output.join(format!("{prefix}.layout.json")),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
            shell.audit_feedback_capture(&self.output, &prefix)?;
            for float in &floats.windows {
                gpu.select_target(float.key);
                gpu.pixels()?
                    .save(self.output.join(format!("{prefix}-{}.png", float.id)))?;
            }
            gpu.select_target(0);
            self.trace.push(json!({"stage":stage,"sceneReady":scene_ready,"pointer":shell.pointer,"dragging":shell.content_dragging(),"drop":shell.content_drag_report(),"layout":shell.layout_report()}));
            if matches!(act, Act::Hierarchy(_)) && self.age >= PREVIEW {
                self.hierarchy_proof[stage] = hierarchy::snapshot(editor)?;
            }
        }
        if self.age >= act.finish() {
            let (ok, observed) = self
                .observe(act, editor, shell)
                .unwrap_or_else(|error| (false, json!({"error":error.to_string()})));
            self.results.push(json!({"behaviour":act.label(),"ok":ok,"beforeRevision":self.before_revision,"afterRevision":editor.host.store().revision(),"observed":observed,"trace":self.trace,"capturePrefix":format!("{:02}-{}", self.index+1,act.label())}));
            shell.desktop_pointer = None;
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
        }
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("behaviours.json"),
            ara_json::to_canonical_string(
                &json!({"behaviours":self.results,"passed":self.results.iter().filter(|result| result["ok"]==true).count(),"failed":self.failures().len(),"completed":self.finished(),
                    "sceneReadinessRequired":true,"sceneWaitLimitSeconds":SCENE_PATIENCE.as_secs(),
                    "current":{"behaviour":ACTS.get(self.index).map(|act|act.label()),"age":self.age,
                        "waitingForScene":self.scene_wait.is_some(),"sceneWaitSeconds":self.scene_wait.map(|started|started.elapsed().as_secs_f64()),
                        "sceneReadinessPolls":self.scene_wait_polls}}),
            ),
        )?;
        let mut html = String::from(
            "<!doctype html><meta charset='utf-8'><title>Ara editor behaviour evidence</title><style>body{font:14px system-ui;background:#17181b;color:#eee;margin:24px}section{margin:24px 0}div{display:flex;gap:12px}figure{margin:0;flex:1;min-width:0}img{width:100%}figcaption{margin-top:6px}</style><h1>Ara preview 4 editor behaviour evidence</h1><p>Hidden native windows; production event router. Exact outcomes and layout bounds are in <a href='behaviours.json'>behaviours.json</a>.</p>",
        );
        for (index, act) in ACTS.iter().enumerate().take(self.index) {
            html.push_str(&format!(
                "<section><h2>{}. {}</h2><div>",
                index + 1,
                act.label()
            ));
            for stage in ["before", "preview", "after"] {
                let file = format!("{:02}-{}-{stage}.png", index + 1, act.label());
                html.push_str(&format!("<figure><a href='{file}'><img src='{file}'></a><figcaption>{stage}</figcaption></figure>"));
            }
            html.push_str("</div></section>");
        }
        std::fs::write(self.output.join("index.html"), html)?;
        Ok(())
    }
}
