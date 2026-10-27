//! Native J1/J4 navigation probe, chained after `--verify-behaviours` on its isolated project.
//! The production event router and fly integrator run normally; held test frames use
//! exactly 1/120 second so speed comparisons have identical input duration.
use super::*;
use ara_ui::paint::Box2;
use std::path::Path;

const HOLD: u32 = 120;
const BEAM: &str = "e_framebeam";
const BOOKMARK: &str = "Audit orthographic detail";
const MODEL: &str = "assets/rock.glb";
const FIXTURE_PATIENCE: Duration = Duration::from_secs(120);

struct ModelSource {
    asset: String,
    digest: ara_asset::Digest,
    publication: u64,
}
struct FixtureWait {
    phase: &'static str,
    started: Instant,
    polls: u64,
    reported: Option<Instant>,
}
#[derive(Clone, Copy)]
enum Act {
    Fly(f32),
    Panel,
    Modal,
    Look,
    Frame,
    Projection,
    Bookmark,
}
const ACTS: &[Act] = &[
    Act::Fly(1.),
    Act::Fly(4.),
    Act::Fly(16.),
    Act::Panel,
    Act::Modal,
    Act::Look,
    Act::Frame,
    Act::Projection,
    Act::Bookmark,
];
impl Act {
    fn label(self) -> String {
        match self {
            Self::Fly(speed) => format!("fly-{speed}"),
            Self::Panel => "panel-owns-input".into(),
            Self::Modal => "modal-owns-input".into(),
            Self::Look => "fly-look-eye".into(),
            Self::Frame => "frame-imported-bounds".into(),
            Self::Projection => "orthographic-toolbar-wheel".into(),
            Self::Bookmark => "bookmark-native-save-recall".into(),
        }
    }
}
pub(super) struct NavigationCheck {
    output: PathBuf,
    index: usize,
    age: u32,
    waiting: bool,
    waited: u32,
    source: [f32; 2],
    before: crate::state::Camera,
    revision: u64,
    frame_ms: Vec<f64>,
    results: Vec<Value>,
    started: Option<Instant>,
    input_to_capture_ms: Option<f64>,
    intermediate: Option<crate::state::Camera>,
    assets: ara_play_host::PublishedAssets,
    asset_snapshot: Option<std::sync::Arc<ara_play_host::PublishedSnapshot>>,
    asset_progress: Option<std::sync::Arc<ara_play_host::ImportProgress>>,
    asset_revision: u64,
    progress_revision: u64,
    model_source: Option<ModelSource>,
    fixture_wait: Option<FixtureWait>,
    fixture_status: Value,
    fixture_trace: Vec<Value>,
}
fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
impl NavigationCheck {
    pub fn new(editor: &Editor, output: &Path) -> ara_ui::Result<Self> {
        Ok(Self {
            output: output.into(),
            index: 0,
            age: 0,
            waiting: false,
            waited: 0,
            source: [0.; 2],
            before: crate::state::Camera::default(),
            revision: 0,
            frame_ms: Vec::new(),
            results: Vec::new(),
            started: None,
            input_to_capture_ms: None,
            intermediate: None,
            assets: ara_play_host::PublishedAssets::for_project(editor.host.root())?,
            asset_snapshot: None,
            asset_progress: None,
            asset_revision: u64::MAX,
            progress_revision: u64::MAX,
            model_source: None,
            fixture_wait: None,
            fixture_status: Value::Null,
            fixture_trace: Vec::new(),
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
    /// Only the test's held input frames override navigation dt; ordinary production runs never do.
    pub fn fly_dt(&self) -> Option<f32> {
        let act = ACTS.get(self.index)?;
        (matches!(act, Act::Fly(_) | Act::Panel | Act::Modal) && (3..3 + HOLD).contains(&self.age))
            .then_some(1. / 120.)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "native loop owners are borrowed independently"
    )]
    pub fn inject(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        scene_view: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if self.age == 0 {
            if matches!(act, Act::Frame) && !self.model_published(editor)? {
                return Ok(());
            }
            shell.leave();
            shell.menu = None;
            shell.dialog = None;
            editor.action("editor.layout.reset", json!({}))?;
            editor.action("editor.select", json!({"entities":[]}))?;
            editor.action("editor.focus", json!({"panel":"scene"}))?;
            let camera = crate::state::Camera::default();
            editor.action("editor.camera",json!({"target":camera.target.to_array(),"yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance,"fovYDegrees":camera.fov_y_degrees,"projection":camera.projection,"orthographicHeight":camera.orthographic_height}))?;
            if matches!(act, Act::Bookmark) {
                editor.action(
                    "editor.camera",
                    json!({"projection":"orthographic","orthographicHeight":6.}),
                )?;
            }
            editor.action("editor.settings.set",json!({"key":"viewport.cameraSpeed","value":if let Act::Fly(speed)=act {speed}else{4.}}))?;
            if matches!(act, Act::Modal) {
                shell.dialog = Some(crate::shell::Dialog::about());
            }
            if matches!(act, Act::Frame) {
                let source = self
                    .model_source
                    .as_ref()
                    .ok_or("J1 published model identity is missing")?;
                let reference = format!("@{MODEL}#{}", source.asset);
                editor.apply(vec![json!({"cmd":"entity.create","args":{"id":BEAM,"name":"Imported long beam","components":{"Transform":{"scale":[80.,0.1,0.1]},"MeshRenderer":{"model":reference}}}})],"Create J1 imported-bounds fixture",None)?;
                editor.action("editor.select", json!({"entities":[BEAM]}))?;
            }
            shell.rebuild(editor)?;
            shell.input.blur(&mut shell.ui);
            self.before = editor.state.camera;
            self.revision = editor.host.store().revision();
            self.source = if matches!(act, Act::Panel) {
                let node = shell
                    .shown_node("header-inspector")
                    .ok_or("J1 Inspector header is not reachable")?;
                centre(shell.ui.nodes[node].rect)
            } else {
                centre(
                    shell
                        .viewport()
                        .ok_or("J1 Scene viewport is not reachable")?,
                )
            };
        } else if self.age == 1 {
            if matches!(act, Act::Frame)
                && !self.native_model_ready(editor, shell, scene_view, scene_ready)?
            {
                return Ok(());
            }
            events.push((
                window.id(),
                Event::MouseMotion {
                    position: self.source.map(|value| value / scale),
                    delta: [0.; 2],
                },
            ));
        } else if matches!(act, Act::Projection | Act::Bookmark) {
            self.camera_input(act, editor, shell, window, scale, events)?;
        } else if self.age == 2 {
            if matches!(act, Act::Frame)
                && !self.native_model_ready(editor, shell, scene_view, scene_ready)?
            {
                return Ok(());
            }
            self.started = Some(Instant::now());
            if matches!(act, Act::Frame) {
                for pressed in [true, false] {
                    events.push((
                        window.id(),
                        Event::Key {
                            scancode: 9,
                            pressed,
                            repeat: false,
                        },
                    ));
                }
            } else {
                events.push((
                    window.id(),
                    Event::MouseButton {
                        button: 3,
                        pressed: true,
                    },
                ));
            }
        } else if self.age == 3 {
            if matches!(act, Act::Look) {
                events.push((
                    window.id(),
                    Event::MouseMotion {
                        position: [
                            (self.source[0] + 40.) / scale,
                            (self.source[1] + 20.) / scale,
                        ],
                        delta: [40. / scale, 20. / scale],
                    },
                ));
            } else if !matches!(act, Act::Frame) {
                events.push((
                    window.id(),
                    Event::Key {
                        scancode: 26,
                        pressed: true,
                        repeat: false,
                    },
                ));
            }
        } else if self.age == 4 && matches!(act, Act::Look) {
            events.push((
                window.id(),
                Event::MouseButton {
                    button: 3,
                    pressed: false,
                },
            ));
        } else if self.age == 3 + HOLD && !matches!(act, Act::Frame | Act::Look) {
            events.push((
                window.id(),
                Event::Key {
                    scancode: 26,
                    pressed: false,
                    repeat: false,
                },
            ));
            events.push((
                window.id(),
                Event::MouseButton {
                    button: 3,
                    pressed: false,
                },
            ));
        }
        Ok(())
    }
    fn camera_input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match (act, self.age) {
            (Act::Projection, 2) => {
                self.started = Some(Instant::now());
                click_control(shell, "view-projection", window, scale, events)?;
            }
            (Act::Projection, 4) | (Act::Bookmark, 7) => {
                events.push((
                    window.id(),
                    Event::MouseMotion {
                        position: self.source.map(|value| value / scale),
                        delta: [0.; 2],
                    },
                ));
                events.push((window.id(), Event::Wheel([0., 3.])));
            }
            (Act::Bookmark, 2) => {
                self.started = Some(Instant::now());
                click_control(shell, "view-bookmark-save", window, scale, events)?;
            }
            (Act::Bookmark, 3) => {
                let field = shell.ui.nodes.iter().find(|node| {
                    node.visible
                        && node.element.id.starts_with("editor-confirm-")
                        && node.element.id.ends_with("-field")
                });
                if field.is_none() {
                    return self.wait_for("Bookmark name prompt did not open");
                }
                events.push((
                    window.id(),
                    Event::TextInput {
                        text: BOOKMARK.into(),
                    },
                ));
                enter(window, events);
            }
            (Act::Bookmark, 5) => {
                let views = editor.action("editor.camera.bookmark", json!({"action":"list"}))?;
                if !views["bookmarks"]
                    .as_array()
                    .is_some_and(|views| views.iter().any(|view| view["name"] == BOOKMARK))
                {
                    return self.wait_for(
                        "Native bookmark Save did not publish its typed navigation edit",
                    );
                }
            }
            (Act::Bookmark, 9) => {
                self.intermediate = Some(editor.state.camera);
                click_control(shell, "view-bookmarks", window, scale, events)?;
            }
            (Act::Bookmark, 10) => {
                let choice = shell.ui.nodes.iter().find(|node| {
                    node.visible
                        && node.element.id.starts_with("editor-confirm-")
                        && node.element.id.ends_with("-choice-0")
                });
                let Some(choice) = choice else {
                    return self.wait_for("Bookmark recall chooser did not open");
                };
                click_control(shell, &choice.element.id, window, scale, events)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn wait_for(&mut self, message: &str) -> ara_ui::Result<()> {
        self.waiting = true;
        self.waited += 1;
        if self.waited > 144 * 30 {
            return Err(message.to_owned().into());
        }
        Ok(())
    }
    fn refresh_assets(&mut self) {
        if self.asset_revision != self.assets.revision()
            && let Some(snapshot) = self.assets.snapshot()
        {
            self.asset_revision = snapshot.revision();
            self.asset_snapshot = Some(snapshot);
        }
        let revision = self.assets.progress_revision();
        if self.progress_revision != revision
            && let Some(progress) = self.assets.progress()
        {
            self.progress_revision = revision;
            self.asset_progress = Some(progress);
        }
    }
    fn model_published(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        self.refresh_assets();
        let asset = editor
            .host
            .store()
            .workspace()
            .index()
            .id_of(MODEL)
            .map(str::to_owned);
        let generation = asset.as_ref().and_then(|asset| {
            let snapshot = self.asset_snapshot.as_ref()?;
            let generation = snapshot.generation(asset)?;
            Some((
                generation.digest(),
                generation.kinds().contains(&ara_asset::ArtifactKind::Mesh),
                snapshot.revision(),
            ))
        });
        let failure = self
            .asset_progress
            .as_ref()
            .and_then(|progress| {
                asset
                    .as_ref()
                    .and_then(|asset| progress.failed.get(asset))
                    .or_else(|| progress.failed.get(MODEL))
            })
            .cloned();
        let status = json!({"path":MODEL,"asset":asset,"generation":generation.map(|(digest,_,_)|digest.to_string()),
            "readerRevision":self.assets.revision(),"snapshotRevision":self.asset_snapshot.as_ref().map(|snapshot|snapshot.revision()),
            "importProgress":self.asset_progress.as_ref().map(|progress|json!({"done":progress.done,"total":progress.total,"pending":progress.pending,"failed":progress.failed}))});
        if let Some(failure) = failure {
            self.fixture_status = status;
            return self.fixture_failure(
                editor,
                &format!("J1 model import failed for {MODEL}: {failure}"),
            );
        }
        let Some((digest, mesh, publication)) = generation else {
            return self.fixture_pending(editor, "model-publication", status);
        };
        if !mesh {
            self.fixture_status = status;
            return self.fixture_failure(
                editor,
                "J1 rock generation published without a mesh artifact",
            );
        }
        let Some(asset) = asset else {
            return self.fixture_pending(editor, "model-publication", status);
        };
        self.model_source = Some(ModelSource {
            asset,
            digest,
            publication,
        });
        self.fixture_ready(editor, "model-publication", status)
    }
    fn imported_bounds(&self, shell: &Shell, scene: &viewport::SceneView) -> Option<[Vec3; 2]> {
        let source = self.model_source.as_ref()?;
        scene.selected_imported_bounds(
            &[BEAM.into()],
            &shell.outlines.channels,
            &source.asset,
            source.digest,
        )
    }
    fn native_model_ready(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        scene: &viewport::SceneView,
        scene_ready: bool,
    ) -> ara_ui::Result<bool> {
        let bounds = self.imported_bounds(shell, scene);
        let status = json!({"sceneReady":scene_ready,"exactImportedGeometry":bounds.is_some(),
            "bounds":bounds.map(|bounds|bounds.map(|point|point.to_array())),"source":self.model_proof()});
        if !scene_ready || bounds.is_none() {
            return self.fixture_pending(editor, "native-model-presentation", status);
        }
        self.fixture_ready(editor, "native-model-presentation", status)
    }
    fn fixture_pending(
        &mut self,
        editor: &mut Editor,
        phase: &'static str,
        status: Value,
    ) -> ara_ui::Result<bool> {
        let now = Instant::now();
        if self
            .fixture_wait
            .as_ref()
            .is_none_or(|wait| wait.phase != phase)
        {
            self.fixture_wait = Some(FixtureWait {
                phase,
                started: now,
                polls: 0,
                reported: None,
            });
            self.fixture_trace.push(json!({"phase":phase,"state":"waiting","limitSeconds":FIXTURE_PATIENCE.as_secs(),"status":status}));
        }
        self.fixture_status = status;
        self.waiting = true;
        let wait = self
            .fixture_wait
            .as_mut()
            .ok_or("J1 fixture wait state is missing")?;
        wait.polls += 1;
        if now.duration_since(wait.started) >= FIXTURE_PATIENCE {
            return self.fixture_failure(
                editor,
                &format!(
                    "J1 {phase} did not become ready within {} real seconds",
                    FIXTURE_PATIENCE.as_secs()
                ),
            );
        }
        if wait
            .reported
            .is_none_or(|reported| now.duration_since(reported) >= Duration::from_secs(1))
        {
            wait.reported = Some(now);
            self.write()?;
        }
        Ok(false)
    }
    fn fixture_ready(
        &mut self,
        editor: &mut Editor,
        phase: &'static str,
        status: Value,
    ) -> ara_ui::Result<bool> {
        self.fixture_status = status;
        if self
            .fixture_wait
            .as_ref()
            .is_some_and(|wait| wait.started.elapsed() >= FIXTURE_PATIENCE)
        {
            return self.fixture_failure(
                editor,
                &format!(
                    "J1 {phase} exceeded its {} real-second readiness deadline",
                    FIXTURE_PATIENCE.as_secs()
                ),
            );
        }
        if let Some(wait) = self.fixture_wait.take() {
            self.fixture_trace.push(json!({"phase":phase,"state":"ready","seconds":wait.started.elapsed().as_secs_f64(),"polls":wait.polls,"status":self.fixture_status}));
        } else {
            self.fixture_trace.push(json!({"phase":phase,"state":"ready","seconds":0.,"polls":0,"status":self.fixture_status}));
        }
        self.waiting = false;
        Ok(true)
    }
    fn fixture_failure(&mut self, editor: &mut Editor, message: &str) -> ara_ui::Result<bool> {
        editor.report_error(message);
        self.fixture_trace.push(json!({"state":"failed","error":message,"status":self.fixture_status,
            "wait":self.fixture_wait.as_ref().map(|wait|json!({"phase":wait.phase,"seconds":wait.started.elapsed().as_secs_f64(),"polls":wait.polls}))}));
        self.results.push(json!({"journey":ACTS[self.index].label(),"ok":false,"error":message,"fixtureStatus":self.fixture_status,"fixtureTrace":self.fixture_trace,"modelProof":self.model_proof()}));
        self.write()?;
        Err(message.to_owned().into())
    }
    fn model_proof(&self) -> Value {
        self.model_source.as_ref().map_or(Value::Null, |source|json!({"path":MODEL,"asset":source.asset,
            "digest":source.digest.to_string(),"publicationRevision":source.publication,
            "nativeProvenance":"Retained Geometry::Model asset/digest matched to the prepared native instance by Draw.entity; procedural placeholders rejected"}))
    }
    /// Read native completed targets after presentation, then record displacement and frame costs.
    pub fn end(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        scene_view: &viewport::SceneView,
        frame_ms: f64,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        if self.waiting {
            return Ok(());
        }
        if matches!(act, Act::Frame)
            && self.age == 1
            && !self.native_model_ready(editor, shell, scene_view, scene_ready)?
        {
            return Ok(());
        }
        if self.fly_dt().is_some() && frame_ms.is_finite() {
            self.frame_ms.push(frame_ms);
        }
        if self.age == 1 {
            self.capture(gpu, &act.label(), "before")?;
        }
        if matches!(act, Act::Bookmark) && matches!(self.age, 2 | 9) {
            self.capture(
                gpu,
                &act.label(),
                if self.age == 2 {
                    "name-dialog"
                } else {
                    "recall-dialog"
                },
            )?;
        }
        let projection = if matches!(act, Act::Frame) {
            self.imported_bounds(shell, scene_view).and_then(|bounds| {
                shell
                    .viewport()
                    .map(|viewport| projected(&editor.state.camera, viewport, bounds))
            })
        } else {
            None
        };
        if matches!(act, Act::Frame) && self.age >= 4 {
            let ready = scene_ready
                && projection
                    .as_ref()
                    .is_some_and(|projection| projection["fits"] == true)
                && editor.state.camera != self.before;
            let status = json!({"sceneReady":scene_ready,"projection":projection,"source":self.model_proof(),"cameraChanged":editor.state.camera!=self.before});
            if !ready {
                self.fixture_pending(editor, "framed-model-presentation", status)?;
                return Ok(());
            }
            self.fixture_ready(editor, "framed-model-presentation", status)?;
        }
        let finish = match act {
            Act::Look => self.age >= 8,
            Act::Frame => {
                self.age >= 4
                    && projection
                        .as_ref()
                        .is_some_and(|report| report["fits"] == true)
                    && editor.state.camera != self.before
            }
            Act::Projection => self.age >= 7,
            Act::Bookmark => self.age >= 14,
            _ => self.age >= 5 + HOLD,
        };
        if !finish {
            self.age += 1;
            if self.age > 144 * 120 {
                return Err(
                    "J1 Frame did not fit its imported native bounds within the bounded journey"
                        .into(),
                );
            }
            return Ok(());
        }
        self.capture(gpu, &act.label(), "after")?;
        let displacement = editor
            .state
            .camera
            .position()
            .distance(self.before.position());
        let (ok, expected) = match act {
            Act::Fly(speed) => ((displacement - speed).abs() < 0.005, json!(speed)),
            Act::Panel | Act::Modal => (editor.state.camera == self.before, json!(0.)),
            Act::Look => (
                displacement < 0.0001 && editor.state.camera.target != self.before.target,
                json!("same eye, changed view direction"),
            ),
            Act::Frame => (
                projection
                    .as_ref()
                    .is_some_and(|report| report["fits"] == true)
                    && editor.host.store().revision() == self.revision,
                json!("every imported bound corner fits both FOV axes"),
            ),
            Act::Projection => (
                editor.state.camera.projection == crate::state::CameraProjection::Orthographic
                    && editor.state.camera.orthographic_height
                        < 2. * self.before.distance
                            * (self.before.fov_y_degrees.to_radians() * 0.5).tan()
                    && editor.host.store().revision() == self.revision,
                json!(
                    "toolbar switches to parallel projection; native wheel changes orthographic height"
                ),
            ),
            Act::Bookmark => (
                editor.state.camera == self.before
                    && self
                        .intermediate
                        .is_some_and(|camera| camera != self.before)
                    && editor
                        .state
                        .camera_bookmarks
                        .get(&editor.state.doc)
                        .and_then(|views| views.get(BOOKMARK))
                        == Some(&self.before)
                    && editor.host.store().revision() == self.revision,
                json!(
                    "native Save prompt and Recall choice restore the complete orthographic camera after wheel navigation"
                ),
            ),
        };
        self.frame_ms.sort_by(f64::total_cmp);
        self.results.push(json!({"journey":act.label(),"ok":ok,"inputSeconds":if matches!(act,Act::Fly(_)|Act::Panel|Act::Modal){1.}else{0.},"expected":expected,"displacementMetres":displacement,"beforeCamera":self.before,"intermediateCamera":self.intermediate,"afterCamera":editor.state.camera,"inputToCaptureMs":self.input_to_capture_ms,"frameP50Ms":self.frame_ms.get(self.frame_ms.len()/2),"frameP95Ms":self.frame_ms.get(self.frame_ms.len()*95/100),"frameMaxMs":self.frame_ms.last(),"projection":projection,
            "sceneReady":scene_ready,"modelProof":if matches!(act,Act::Frame){self.model_proof()}else{Value::Null},"fixtureTrace":self.fixture_trace}));
        self.index += 1;
        self.age = 0;
        self.waited = 0;
        self.started = None;
        self.input_to_capture_ms = None;
        self.frame_ms.clear();
        self.intermediate = None;
        self.fixture_wait = None;
        self.fixture_status = Value::Null;
        self.fixture_trace.clear();
        self.write()?;
        Ok(())
    }
    fn capture(&mut self, gpu: &mut NativeUi, label: &str, stage: &str) -> ara_ui::Result<()> {
        gpu.select_target(0);
        let pixels = gpu.pixels()?;
        if self.input_to_capture_ms.is_none() {
            self.input_to_capture_ms = self
                .started
                .map(|started| started.elapsed().as_secs_f64() * 1000.);
        }
        pixels.save(
            self.output
                .join(format!("nav-{:02}-{label}-{stage}.png", self.index + 1)),
        )?;
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        std::fs::write(
            self.output.join("navigation.json"),
            ara_json::to_canonical_string(
                &json!({"journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len(),
                    "current":{"journey":ACTS.get(self.index).map(|act|act.label()),"age":self.age,"fixtureStatus":self.fixture_status,
                        "wait":self.fixture_wait.as_ref().map(|wait|json!({"phase":wait.phase,"seconds":wait.started.elapsed().as_secs_f64(),"polls":wait.polls,"limitSeconds":FIXTURE_PATIENCE.as_secs()}))}}),
            ),
        )?;
        Ok(())
    }
}
fn click_control(
    shell: &Shell,
    id: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("Native camera control {id} is not visible"))?;
    let point = centre(shell.ui.nodes[index].rect).map(|value| value / scale);
    events.push((
        window.id(),
        Event::MouseMotion {
            position: point,
            delta: [0.; 2],
        },
    ));
    for pressed in [true, false] {
        events.push((window.id(), Event::MouseButton { button: 1, pressed }));
    }
    Ok(())
}
fn enter(window: &Window, events: &mut Vec<(u32, Event)>) {
    for pressed in [true, false] {
        events.push((
            window.id(),
            Event::Key {
                scancode: 40,
                pressed,
                repeat: false,
            },
        ));
    }
}
fn projected(camera: &crate::state::Camera, viewport: Box2, bounds: [Vec3; 2]) -> Value {
    let Ok(view) = camera
        .spec()
        .resolve([viewport.w.max(1.) as u32, viewport.h.max(1.) as u32])
    else {
        return json!({"fits":false});
    };
    let vp = view.projection * view.view;
    let mut extent = Vec3::ZERO;
    let mut visible = true;
    for index in 0..8 {
        let point = Vec3::new(
            if index & 1 == 0 {
                bounds[0].x
            } else {
                bounds[1].x
            },
            if index & 2 == 0 {
                bounds[0].y
            } else {
                bounds[1].y
            },
            if index & 4 == 0 {
                bounds[0].z
            } else {
                bounds[1].z
            },
        );
        let clip = vp * point.extend(1.);
        visible &= clip.is_finite() && clip.w > 0.;
        extent = extent.max((clip.truncate() / clip.w).abs());
    }
    json!({"fits":visible && extent.is_finite() && extent.x<=0.96 && extent.y<=0.96,"maxNdc":extent.to_array(),"bounds":[bounds[0].to_array(),bounds[1].to_array()],"aspect":viewport.w/viewport.h})
}
