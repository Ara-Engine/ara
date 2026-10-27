//! Focused J1 native input probe, chained after `--verify-behaviours` on its isolated project.
//! The production event router and fly integrator run normally; held test frames use
//! exactly 1/120 second so speed comparisons have identical input duration.
use super::*;
use ara_ui::paint::Box2;
use std::path::Path;

const HOLD: u32 = 120;
const BEAM: &str = "e_framebeam";
#[derive(Clone, Copy)]
enum Act {
    Fly(f32),
    Panel,
    Modal,
    Look,
    Frame,
}
const ACTS: &[Act] = &[
    Act::Fly(1.),
    Act::Fly(4.),
    Act::Fly(16.),
    Act::Panel,
    Act::Modal,
    Act::Look,
    Act::Frame,
];
impl Act {
    fn label(self) -> String {
        match self {
            Self::Fly(speed) => format!("fly-{speed}"),
            Self::Panel => "panel-owns-input".into(),
            Self::Modal => "modal-owns-input".into(),
            Self::Look => "fly-look-eye".into(),
            Self::Frame => "frame-imported-bounds".into(),
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
}
fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
impl NavigationCheck {
    pub fn new(output: &Path) -> Self {
        Self {
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
        }
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
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        self.waiting = false;
        if self.age == 0 {
            shell.leave();
            shell.menu = None;
            shell.dialog = None;
            editor.action("editor.layout.reset", json!({}))?;
            editor.action("editor.select", json!({"entities":[]}))?;
            editor.action("editor.focus", json!({"panel":"scene"}))?;
            let camera = crate::state::Camera::default();
            editor.action("editor.camera",json!({"target":camera.target.to_array(),"yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance,"fovYDegrees":camera.fov_y_degrees}))?;
            editor.action("editor.settings.set",json!({"key":"viewport.cameraSpeed","value":if let Act::Fly(speed)=act {speed}else{4.}}))?;
            if matches!(act, Act::Modal) {
                shell.dialog = Some(crate::shell::Dialog::about());
            }
            if matches!(act, Act::Frame) {
                let asset = editor
                    .host
                    .store()
                    .workspace()
                    .index()
                    .id_of("assets/rock.glb")
                    .ok_or("J1 imported model fixture has not published")?;
                let reference = format!("@assets/rock.glb#{asset}");
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
                && scene_view
                    .selected_bounds(&[BEAM.into()], &shell.outlines.channels)
                    .is_none()
            {
                self.waiting = true;
                self.waited += 1;
                if self.waited > 144 * 120 {
                    return Err("J1 imported native geometry did not become available".into());
                }
                return Ok(());
            }
            events.push((
                window.id(),
                Event::MouseMotion {
                    position: self.source.map(|value| value / scale),
                    delta: [0.; 2],
                },
            ));
        } else if self.age == 2 {
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
    /// Read native completed targets after presentation, then record displacement and frame costs.
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        scene_view: &viewport::SceneView,
        frame_ms: f64,
    ) -> ara_ui::Result<()> {
        let Some(&act) = ACTS.get(self.index) else {
            return Ok(());
        };
        if self.waiting {
            return Ok(());
        }
        if self.fly_dt().is_some() && frame_ms.is_finite() {
            self.frame_ms.push(frame_ms);
        }
        if self.age == 1 {
            self.capture(gpu, &act.label(), "before")?;
        }
        let projection = if matches!(act, Act::Frame) {
            scene_view
                .selected_bounds(&[BEAM.into()], &shell.outlines.channels)
                .and_then(|bounds| {
                    shell
                        .viewport()
                        .map(|viewport| projected(&editor.state.camera, viewport, bounds))
                })
        } else {
            None
        };
        let finish = match act {
            Act::Look => self.age >= 8,
            Act::Frame => {
                self.age >= 4
                    && projection
                        .as_ref()
                        .is_some_and(|report| report["fits"] == true)
                    && editor.state.camera != self.before
            }
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
        };
        self.frame_ms.sort_by(f64::total_cmp);
        self.results.push(json!({"journey":act.label(),"ok":ok,"inputSeconds":if matches!(act,Act::Fly(_)|Act::Panel|Act::Modal){1.}else{0.},"expected":expected,"displacementMetres":displacement,"beforeCamera":self.before,"afterCamera":editor.state.camera,"inputToCaptureMs":self.input_to_capture_ms,"frameP50Ms":self.frame_ms.get(self.frame_ms.len()/2),"frameP95Ms":self.frame_ms.get(self.frame_ms.len()*95/100),"frameMaxMs":self.frame_ms.last(),"projection":projection}));
        self.index += 1;
        self.age = 0;
        self.waited = 0;
        self.started = None;
        self.input_to_capture_ms = None;
        self.frame_ms.clear();
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
                &json!({"journeys":self.results,"completed":self.finished(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),"failed":self.failures().len()}),
            ),
        )?;
        Ok(())
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
