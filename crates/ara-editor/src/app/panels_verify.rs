//! Bounded native authoring route: real imports, UI drags, captures and 500-node navigation.
use super::*;
use ara_platform::Event;
use std::collections::BTreeMap;

pub(super) struct Verification {
    phase: u32,
    age: u32,
    phase_started: Instant,
    job: Option<String>,
    material: String,
    vfx: String,
    animation: String,
    output: PathBuf,
    pixels: [u32; 2],
    pub samples: Vec<f64>,
    stage_samples: Vec<[f64; 5]>,
    bench_frames: u32,
    interactions: Vec<&'static str>,
    /// The Add node search popup has been captured.
    palette_shot: bool,
    /// The plain material page has been captured; the in-node previews come next.
    material_shot: bool,
    /// Gizmo hover/drag state for the Scene captures.
    gizmo: overlay::GizmoState,
}

pub(super) fn fixtures(root: &std::path::Path) -> ara_ui::Result<()> {
    if !root.join("ara.project.json").exists() {
        std::fs::create_dir_all(root.join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(&json!({"name":"Outpost"})),
        )?;
        std::fs::write(
            root.join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_golden00","records":{}}),
            ),
        )?;
    }
    std::fs::create_dir_all(root.join("models"))?;
    std::fs::create_dir_all(root.join("textures"))?;
    let mesh = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ara-render/examples/assets/Avocado.glb");
    if !root.join("models/Avocado.glb").exists() {
        std::fs::copy(mesh, root.join("models/Avocado.glb"))?;
    }
    let texture = root.join("textures/checker.png");
    if !texture.exists() {
        image::RgbaImage::from_fn(128, 128, |x, y| {
            image::Rgba(if (x / 16 + y / 16) % 2 == 0 {
                [54, 133, 214, 255]
            } else {
                [215, 230, 246, 255]
            })
        })
        .save(texture)?;
    }
    Ok(())
}
pub(super) fn resource(editor: &mut Editor, domain: &str) -> ara_ui::Result<String> {
    let preferred = format!(
        "samples/{domain}.{}",
        if domain == "animation" {
            "animation"
        } else {
            "graph"
        }
    );
    if let Some(id) = editor.host.store().workspace().find(&preferred) {
        return Ok(id.to_string());
    }
    if let Some((_, doc)) = editor.host.store().workspace().docs().find(|(_, d)| {
        if domain == "animation" {
            d.kind == "animation"
        } else {
            d.record("g_meta")
                .is_some_and(|r| r.get("domain") == Some(&json!(domain)))
        }
    }) {
        return Ok(doc.id.to_string());
    }
    let path = format!(
        "samples/{domain}.{}",
        if domain == "animation" {
            "animation"
        } else {
            "graph"
        }
    );
    Ok(editor.action(
        "editor.resource.create",
        json!({"path":path,"domain":domain}),
    )?["doc"]
        .as_str()
        .ok_or("sample resource identity")?
        .to_owned())
}
impl Verification {
    pub fn new(
        editor: &mut Editor,
        output: &std::path::Path,
        pixels: [u32; 2],
    ) -> ara_ui::Result<Self> {
        editor.action("editor.preferences", json!({"reduceMotion":true}))?;
        editor.action("editor.theme", json!({"light":false}))?;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.workspace", json!({"playing":true,"compare":false}))?;
        let material = resource(editor, "material")?;
        let vfx = resource(editor, "vfx")?;
        let animation = resource(editor, "animation")?;
        let controller = json!({"initial":"Idle","parameters":{"speed":{}},"nodes":{"idle":{"clip":null},"run":{"clip":null},"locomotion":{"kind":"blend1d","parameterX":"speed","samples":[{"node":"idle","x":0},{"node":"run","x":1}]}},"states":{"Idle":{"node":"idle","duration":48000},"Run":{"node":"run","duration":24000},"Locomotion":{"node":"locomotion","duration":48000}},"transitions":[{"id":"idle_to_run","from":"Idle","to":"Run","duration":4800,"conditions":[{"parameter":"speed","comparison":"greater","floatValue":0.2}]},{"id":"run_to_idle","from":"Run","to":"Idle","duration":4800}]});
        editor.apply_to(&animation,vec![json!({"cmd":"editor.animation.controller.set","args":{"record":"an_controller","controller":controller}})],"Prepare animation panel verification")?;
        editor.action("editor.focus", json!({"panel":"scene"}))?;
        editor.action("editor.focus", json!({"panel":"assets"}))?;
        let job =
            editor.action("editor.asset.import", json!({"path":"models/Avocado.glb"}))?["job"]
                .as_str()
                .map(str::to_owned);
        Ok(Self {
            phase: 0,
            age: 0,
            phase_started: Instant::now(),
            job,
            material,
            vfx,
            animation,
            output: output.into(),
            pixels,
            samples: Vec::new(),
            stage_samples: Vec::new(),
            bench_frames: 0,
            interactions: Vec::new(),
            palette_shot: false,
            material_shot: false,
            gizmo: Default::default(),
        })
    }
    pub fn finished(&self) -> bool {
        self.phase >= 14
    }
    pub fn benchmarking(&self) -> bool {
        self.phase == 13
    }
    pub fn record(&mut self, ms: f64, cumulative: [f64; 4]) {
        if self.phase == 13 {
            if self.bench_frames >= 120 {
                self.samples.push(ms);
                let [control, paint, gpu, present] = cumulative;
                self.stage_samples.push([
                    control,
                    paint - control,
                    gpu - paint,
                    present - gpu,
                    ms - present,
                ]);
            }
            self.bench_frames += 1;
        }
    }
    fn next(&mut self) {
        eprintln!("panel verification completed phase {}", self.phase);
        self.phase += 1;
        self.age = 0;
        self.phase_started = Instant::now();
    }
    fn job_done(&self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(job) = &self.job else {
            return Ok(true);
        };
        let status = editor.action("jobs.status", json!({"job":job}))?;
        match status["status"].as_str() {
            Some("done") => Ok(true),
            Some("failed" | "cancelled") => {
                Err(format!("panel verification import failed: {status}").into())
            }
            _ => Ok(false),
        }
    }
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
        self.age += 1;
        if self.phase_started.elapsed() > Duration::from_secs(90) {
            let status = self
                .job
                .as_ref()
                .and_then(|job| editor.action("jobs.status", json!({"job":job})).ok());
            return Err(format!(
                "panel verification timed out at phase {} after {:?}: {status:?}",
                self.phase,
                self.phase_started.elapsed()
            )
            .into());
        }
        let mut clipboard = OsClipboard;
        match self.phase {
            0 => {
                if !self.job_done(editor)? {
                    return Ok(false);
                }
                self.interactions.push("isolated mesh import completed");
                self.job = editor.action(
                    "editor.asset.import",
                    json!({"path":"textures/checker.png"}),
                )?["job"]
                    .as_str()
                    .map(str::to_owned);
                self.next();
            }
            1 => {
                if !self.job_done(editor)? {
                    return Ok(false);
                }
                editor.action(
                    "editor.workspace",
                    json!({"asset":"models/Avocado.glb","folder":"models"}),
                )?;
                shell.rebuild(editor)?;
                self.next();
            }
            2 => {
                let Some(point) = shell.asset_point("models/Avocado.glb") else {
                    return Ok(false);
                };
                let rect = shell.viewport().ok_or("asset drag scene viewport")?;
                let before = editor.host.store().revision();
                shell.pointer = point;
                shell.begin_drag();
                shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
                if !shell.drop_at(editor)? || editor.host.store().revision() != before + 1 {
                    return Err("asset drag did not produce one transaction".into());
                }
                self.interactions
                    .push("thumbnail asset drag instantiated mesh through typed ops");
                editor.action("history.undo", json!({}))?;
                editor.action("history.redo", json!({}))?;
                let doc = editor
                    .host
                    .store()
                    .workspace()
                    .find("models/Avocado.glb")
                    .ok_or("import settings doc")?
                    .to_string();
                editor.apply_to(&doc,vec![json!({"cmd":"import.configure","args":{"source":"models/Avocado.glb","settings":{"lodCount":2},"preset":"fast"}})],"Verify reimport settings")?;
                self.job = editor.action(
                    "editor.asset.reimport",
                    json!({"path":"models/Avocado.glb"}),
                )?["job"]
                    .as_str()
                    .map(str::to_owned);
                self.next();
            }
            3 => {
                if !self.job_done(editor)? {
                    return Ok(false);
                }
                // Assets navigates folders (UX v2): the models folder shows the mesh
                // tile and its import preview, then samples shows the material and
                // VFX tiles. The project root lists only folders.
                if editor.state.asset_folder == "models" {
                    shell.rebuild(editor)?;
                    if self.age < 240 || !shell.asset_previews_ready(&["models/Avocado.glb"]) {
                        return Ok(false);
                    }
                    self.interactions
                        .push("settings edit and isolated reimport completed");
                    let grid =
                        shell.ui.nodes[shell.ui.find("asset-grid").ok_or("asset grid")?].rect;
                    let caption = shell
                        .ui
                        .nodes
                        .iter()
                        .find(|n| {
                            n.element.id.starts_with("asset-card-")
                                && n.element.id.ends_with("-name")
                                && n.text.as_ref().is_some_and(|t| t.value == "Avocado.glb")
                        })
                        .ok_or("mesh filename caption")?
                        .rect;
                    if !grid.contains(caption.x, caption.y)
                        || !grid.contains(caption.x + caption.w - 0.1, caption.y + caption.h - 0.1)
                    {
                        return Err("asset filename is clipped in the short drawer".into());
                    }
                    self.capture_pair("assets", editor, shell, gpu, ui, renderer, scene_view)?;
                    editor.action(
                        "editor.workspace",
                        json!({"asset":"samples/material.graph","folder":"samples"}),
                    )?;
                    shell.rebuild(editor)?;
                    return Ok(false);
                }
                shell.rebuild(editor)?;
                if !shell.asset_previews_ready(&["samples/material.graph", "samples/vfx.graph"]) {
                    return Ok(false);
                }
                self.capture_pair(
                    "assets-samples",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene_view,
                )?;
                editor.action(
                    "editor.workspace",
                    json!({"resource":self.material,"asset":""}),
                )?;
                shell.rebuild(editor)?;
                self.next();
            }
            4 => {
                if !shell.preview_ready("material") {
                    return Ok(false);
                }
                // The Add node popup, unfiltered, before the scripted interaction.
                if !self.palette_shot {
                    if !editor.state.canvas_add {
                        shell.click("graph-add", &mut clipboard)?;
                        shell.dispatch(editor)?;
                        shell.rebuild(editor)?;
                        return Ok(false);
                    }
                    self.capture_pair(
                        "material-add",
                        editor,
                        shell,
                        gpu,
                        ui,
                        renderer,
                        scene_view,
                    )?;
                    self.palette_shot = true;
                    editor.action("editor.canvas", json!({"add":false}))?;
                    shell.rebuild(editor)?;
                    return Ok(false);
                }
                self.material_interaction(editor, shell, &mut clipboard)?;
                self.next();
            }
            5 => {
                if !shell.preview_ready("material") || self.age < 30 {
                    return Ok(false);
                }
                if !self.material_shot {
                    self.capture_pair("material", editor, shell, gpu, ui, renderer, scene_view)?;
                    self.material_shot = true;
                    // Then a richer graph, authored as text, for the in-node previews.
                    editor.apply_to(
                        &self.material,
                        vec![json!({"cmd":"material.text.apply","args":{"text":crate::asset_tools::node_preview::SHOWCASE}})],
                        "Show in-node previews",
                    )?;
                    shell.rebuild(editor)?;
                    shell.frame_graph(editor)?;
                    shell.rebuild(editor)?;
                    return Ok(false);
                }
                if !shell.node_previews_ready() {
                    return Ok(false);
                }
                self.capture_pair(
                    "material-previews",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene_view,
                )?;
                editor.action(
                    "editor.workspace",
                    json!({"resource":self.vfx,"item":"flame|update|curl"}),
                )?;
                shell.rebuild(editor)?;
                self.next();
            }
            6 => {
                if !shell.preview_ready("vfx") || self.age < 90 {
                    return Ok(false);
                }
                editor.action("editor.workspace", json!({"playing":false}))?;
                shell.rebuild(editor)?;
                self.capture_pair("vfx", editor, shell, gpu, ui, renderer, scene_view)?;
                editor.action(
                    "editor.workspace",
                    json!({"resource":self.animation,"item":"","blends":false}),
                )?;
                shell.rebuild(editor)?;
                self.next();
            }
            7 => {
                let projected = crate::authoring::animation::projection(
                    editor
                        .host
                        .store()
                        .workspace()
                        .doc(&self.animation)
                        .ok_or("animation sample")?,
                    Default::default(),
                    false,
                )?;
                if let Some((id, _)) = projected
                    .nodes
                    .iter()
                    .find(|(_, n)| n.payload["state"] == "Idle")
                {
                    editor.action("editor.canvas", json!({"selection":[id]}))?;
                    shell.rebuild(editor)?;
                }
                self.capture_pair(
                    "animation-states",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene_view,
                )?;
                // Switch views through the toolbar, as a person would.
                shell.click("animation-blends", &mut clipboard)?;
                shell.dispatch(editor)?;
                shell.rebuild(editor)?;
                self.next();
            }
            8 => {
                let projected = crate::authoring::animation::projection(
                    editor
                        .host
                        .store()
                        .workspace()
                        .doc(&self.animation)
                        .ok_or("animation sample")?,
                    Default::default(),
                    true,
                )?;
                if let Some((id, _)) = projected
                    .nodes
                    .iter()
                    .find(|(_, n)| n.payload["pose"] == "locomotion")
                {
                    editor.action("editor.canvas", json!({"selection":[id]}))?;
                    shell.rebuild(editor)?;
                }
                self.capture_pair(
                    "animation-blends",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene_view,
                )?;
                editor.action("editor.focus", json!({"panel":"scene"}))?;
                let entity = crate::world::resolve(editor.host.store(), &editor.state.doc)
                    .map_err(|d| format!("{d:?}"))?
                    .iter()
                    .find(|(_, e)| e.record.get("name") == Some(&json!("Salvage crate")))
                    .map(|(id, _)| id.clone());
                if let Some(entity) = entity {
                    editor.action("editor.select", json!({"entities":[entity]}))?;
                }
                shell.rebuild(editor)?;
                self.capture_pair("scene", editor, shell, gpu, ui, renderer, scene_view)?;
                self.scene_states(editor, shell, gpu, ui, renderer, scene_view)?;
                editor.action("profile.start", json!({}))?;
                shell.rebuild(editor)?;
                self.next();
            }
            9 => {
                let mut camera = editor.state.camera;
                camera.yaw += 0.002;
                set_camera(editor, &camera)?;
                if self.age >= 120 {
                    editor.action("profile.stop", json!({}))?;
                    editor.action("editor.profile.freeze", json!({"baseline":true}))?;
                    editor.action("profile.start", json!({}))?;
                    self.next();
                }
            }
            10 => {
                let mut camera = editor.state.camera;
                camera.yaw += 0.002;
                set_camera(editor, &camera)?;
                if self.age >= 120 {
                    editor.action("profile.stop", json!({}))?;
                    editor.action("editor.profile.freeze", json!({}))?;
                    editor.action("editor.focus", json!({"panel":"profiler"}))?;
                    shell.rebuild(editor)?;
                    self.next();
                }
            }
            11 => {
                if editor.profile_current.is_none() {
                    return Ok(false);
                }
                self.capture_pair("profiler", editor, shell, gpu, ui, renderer, scene_view)?;
                editor.action("editor.workspace", json!({"compare":true}))?;
                shell.rebuild(editor)?;
                self.capture_pair(
                    "profiler-compare",
                    editor,
                    shell,
                    gpu,
                    ui,
                    renderer,
                    scene_view,
                )?;
                self.interactions
                    .push("two real frame/GPU captures frozen and compared");
                self.tool_panels(editor, shell, gpu, ui, renderer, scene_view)?;
                self.load_large_graph(editor, shell)?;
                self.next();
            }
            12 => {
                // Warm samples start after this graph's shader, rather than a last-good image, is ready.
                if self.age < 180 || !shell.preview_ready("material") {
                    return Ok(false);
                }
                let rect = shell
                    .ui
                    .find("graph-canvas")
                    .map(|i| shell.ui.nodes[i].rect)
                    .ok_or("benchmark canvas")?;
                shell.pointer = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
                shell.graph_event(
                    editor,
                    &Event::MouseButton {
                        button: 2,
                        pressed: true,
                    },
                )?;
                self.next();
            }
            13 => {
                if self.bench_frames >= 1120 {
                    shell.graph_event(
                        editor,
                        &Event::MouseButton {
                            button: 2,
                            pressed: false,
                        },
                    )?;
                    self.capture_pair("graph-500", editor, shell, gpu, ui, renderer, scene_view)?;
                    let timings = statistics(&self.samples);
                    let passed = timings["p95"].as_f64().is_some_and(|value| value < 4.);
                    let stages = [
                        "input",
                        "paint",
                        "compileUploadSubmit",
                        "present",
                        "accessibility",
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| {
                        let values = self
                            .stage_samples
                            .iter()
                            .map(|s| s[index])
                            .collect::<Vec<_>>();
                        (name, statistics(&values))
                    })
                    .collect::<BTreeMap<_, _>>();
                    std::fs::write(
                        self.output.join("panel-interactions.json"),
                        ara_json::to_canonical_string(
                            &json!({"verified":true,"flow":self.interactions,"graphNodes":500,"panZoomInputToPresentMs":timings,"panZoomBudgetMs":4,"panZoomBudgetPassed":passed,"profile":if cfg!(debug_assertions){"dev"}else{"release"},"stagesMs":stages,"stageSamplesMs":self.stage_samples,"samplesMs":self.samples}),
                        ),
                    )?;
                    if !passed && !cfg!(debug_assertions) {
                        return Err("500-node pan/zoom p95 exceeded the 4 ms release budget".into());
                    }
                    self.next();
                    return Ok(true);
                }
                let rect = shell
                    .ui
                    .find("graph-canvas")
                    .map(|i| shell.ui.nodes[i].rect)
                    .ok_or("benchmark canvas")?;
                let t = self.bench_frames as f32 * 0.017;
                let position = [
                    rect.x + rect.w * 0.5 + t.sin() * 240.,
                    rect.y + rect.h * 0.5 + t.cos() * 160.,
                ];
                shell.graph_event(
                    editor,
                    &Event::MouseMotion {
                        position,
                        delta: [0., 0.],
                    },
                )?;
                if self.bench_frames.is_multiple_of(8) {
                    shell.graph_event(
                        editor,
                        &Event::Wheel([
                            0.,
                            if self.bench_frames % 240 < 120 {
                                0.4
                            } else {
                                -0.4
                            },
                        ]),
                    )?;
                }
            }
            _ => {}
        }
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    fn capture_pair(
        &self,
        name: &str,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        let actual = shell.ui.size.map(|v| (v * gpu.dpi()).round() as u32);
        if actual != self.pixels {
            return Err(format!("capture requested {:?}, actual {actual:?}", self.pixels).into());
        }
        let capture = super::verify::Verification::new(&self.output).with_gizmo(self.gizmo);
        for (theme, light) in [("dark", false), ("light", true)] {
            editor.action("editor.theme", json!({"light":light}))?;
            shell.rebuild(editor)?;
            shell.upload_thumbnails(ui)?;
            capture.screenshot(
                &format!("{name}-{theme}"),
                editor,
                shell,
                gpu,
                ui,
                renderer,
                scene_view,
            )?;
        }
        editor.action("editor.theme", json!({"light":false}))?;
        shell.rebuild(editor)?;
        crate::shell::icons::install(ui, gpu.dpi())?;
        Ok(())
    }
    /// The Scene view's gizmo states (hovered axis, dragged plane, rotate, scale) and
    /// the viewport toolbar's view-mode and camera menus, with the crate selected.
    #[allow(clippy::too_many_arguments)]
    fn scene_states(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        use crate::shell::commands::{Target, menus::Context};
        use overlay::{GizmoState, Handle};
        for (name, gizmo) in [
            (
                "scene-hover",
                GizmoState {
                    hover: Some(Handle::Axis(0)),
                    active: None,
                },
            ),
            (
                "scene-drag",
                GizmoState {
                    hover: None,
                    active: Some(Handle::Plane(1)),
                },
            ),
        ] {
            self.gizmo = gizmo;
            self.capture_pair(name, editor, shell, gpu, ui, renderer, scene_view)?;
        }
        self.gizmo = Default::default();
        for (name, command) in [
            ("scene-rotate", "view.tool.rotate"),
            ("scene-scale", "view.tool.scale"),
        ] {
            shell.run_command(editor, command, &Target::None)?;
            shell.rebuild(editor)?;
            self.capture_pair(name, editor, shell, gpu, ui, renderer, scene_view)?;
        }
        shell.run_command(editor, "view.tool.move", &Target::None)?;
        for (name, button, context) in [
            ("scene-menu-view", "view-mode", Context::ViewModes),
            ("scene-menu-camera", "view-camera", Context::Camera),
        ] {
            shell.rebuild(editor)?;
            let r = shell
                .ui
                .find(button)
                .map(|i| shell.ui.nodes[i].rect)
                .ok_or("viewport menu button not visible")?;
            shell.pointer = [r.x, r.y + r.h];
            shell.open_viewport_menu(editor, context, Target::None)?;
            self.capture_pair(name, editor, shell, gpu, ui, renderer, scene_view)?;
            shell.menu = None;
        }
        shell.rebuild(editor)?;
        self.interactions
            .push("Scene gizmo states and viewport menus captured");
        Ok(())
    }
    /// Console, Problems, Agents and Settings with real content: diagnostics on the
    /// bus (an import failure with its fix, a C# allocation on the tick path, an
    /// unknown field with a rename), an agent's presence and three turns through the
    /// Engine Protocol, then Preferences, Project and Keyboard shortcuts docked in the
    /// document area.
    #[allow(clippy::too_many_arguments)]
    fn tool_panels(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        // Protocol calls as an agent session would make them.
        let call = |editor: &mut Editor, method: &str, params: Value| {
            ara_proto::Endpoint::call(editor, method, params).map_err(|e| e.message)
        };
        let doc = editor.state.doc.clone();
        let entity = crate::world::resolve(editor.host.store(), &doc)
            .map_err(|d| format!("{d:?}"))?
            .iter()
            .find(|(_, e)| e.record.get("name") == Some(&json!("Salvage crate")))
            .map(|(id, _)| id.clone())
            .ok_or("Salvage crate")?;
        editor.action("diag.report", json!({"engine": true, "diagnostics": [
            {"code":"ARA2101","severity":"error","message":"Import of `models/Crate.fbx` failed: FBX 6.1 binary files are not supported","location":{"file":"models/Crate.fbx"},"fixes":[{"title":"Convert to glTF","action":"cli","args":["ara","import","models/Crate.fbx","--as","gltf"]}]},
            {"code":"ARA0300","severity":"warning","message":"`Spinner.Tick` allocates a `List<int>` every tick","location":{"file":"scripts/Outpost/Spinner.cs","line":42}},
            {"code":"ARA1101","severity":"warning","message":"Unknown field `positon` in `Transform`","location":{"doc":doc,"record":entity,"path":"Transform.positon"},"fixes":[{"title":"Rename `positon` to `position`","action":"rename","record":entity,"path":"Transform.positon","to":"Transform.position"}]},
            {"code":"ARA3309","severity":"info","message":"`Salvage crate` is beyond its draw distance (212 m > 200 m)","location":{"doc":doc,"record":entity}}
        ]}))?;
        let session = call(
            editor,
            "session.begin",
            json!({"client":"panels-verification","author":"agent:claude"}),
        )?["session"]
            .as_str()
            .ok_or("agent session")?
            .to_owned();
        call(
            editor,
            "editor.select",
            json!({"session":session,"entities":[entity]}),
        )?;
        for (why, command) in [
            (
                "Move the salvage crate clear of the patrol route",
                json!({"cmd":"property.set","args":{"entity":entity,"path":"Transform.position.x","value":3.5}}),
            ),
            (
                "Add a waypoint for the dusk patrol",
                json!({"cmd":"entity.create","args":{"id":"$waypoint","name":"Patrol waypoint","components":{"Transform":{"position":[6,0,-4]}}}}),
            ),
            (
                "Turn the crate to face the gate",
                json!({"cmd":"property.set","args":{"entity":entity,"path":"Transform.rotation.y","value":35}}),
            ),
        ] {
            call(
                editor,
                "cmd.apply",
                json!({"session":session,"doc":doc,"why":why,"commands":[command]}),
            )?;
        }
        // A frame boundary, as in the window loop: the agent turns get their time.
        editor.tick();
        for (panel, name) in [
            ("console", "console"),
            ("problems", "problems"),
            ("agents", "agents"),
        ] {
            editor.action("editor.focus", json!({ "panel": panel }))?;
            shell.rebuild(editor)?;
            self.capture_pair(name, editor, shell, gpu, ui, renderer, scene_view)?;
        }
        editor.action(
            "editor.layout.move",
            json!({"panel":"settings","slot":"view"}),
        )?;
        for (name, scope, category) in [
            ("settings", "user", ""),
            ("settings-project", "project", ""),
            ("shortcuts", "user", "Keyboard shortcuts"),
        ] {
            editor.action(
                "editor.settings.view",
                json!({"scope": scope, "category": category}),
            )?;
            editor.action("editor.focus", json!({"panel":"settings"}))?;
            shell.rebuild(editor)?;
            self.capture_pair(name, editor, shell, gpu, ui, renderer, scene_view)?;
        }
        editor.action("editor.layout.close", json!({"panel":"settings"}))?;
        self.interactions
            .push("Console, Problems, Agents and Settings captured with real content");
        Ok(())
    }
    fn material_interaction(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        clipboard: &mut OsClipboard,
    ) -> ara_ui::Result<()> {
        shell.click("graph-add", clipboard)?;
        shell.dispatch(editor)?;
        editor.action(
            "editor.search",
            json!({"panel":"nodes","query":"material.add"}),
        )?;
        shell.rebuild(editor)?;
        let before = editor.host.store().revision();
        shell.click("node-add-0", clipboard)?;
        shell.dispatch(editor)?;
        if editor.host.store().revision() != before + 1 {
            return Err("node palette did not create a node transaction".into());
        }
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&self.material)
                .ok_or("material")?,
        )
        .map_err(|d| d.to_string())?;
        let from = graph
            .nodes
            .iter()
            .find(|(_, n)| n.payload["name"] == "roughness")
            .map(|(id, _)| ara_graph::Endpoint {
                node: id.clone(),
                port: "out".into(),
            })
            .ok_or("roughness port")?;
        let to = graph
            .nodes
            .iter()
            .find(|(_, n)| n.kind == "material.add")
            .map(|(id, _)| ara_graph::Endpoint {
                node: id.clone(),
                port: "in0".into(),
            })
            .ok_or("add port")?;
        editor.action("editor.canvas", json!({"zoom":0.45,"pan":[24,24]}))?;
        shell.sync_canvas(editor);
        shell.frame()?;
        for (endpoint, pressed) in [(&from, true), (&to, false)] {
            let position = shell.graph_port(endpoint).ok_or("live socket")?;
            shell.pointer = position;
            shell.graph_event(
                editor,
                &Event::MouseMotion {
                    position,
                    delta: [0., 0.],
                },
            )?;
            shell.graph_event(editor, &Event::MouseButton { button: 1, pressed })?;
        }
        let after = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(&self.material)
                .ok_or("material")?,
        )
        .map_err(|d| d.to_string())?;
        if !after.edges.values().any(|e| e.from == from && e.to == to) {
            return Err("UI socket connection missing".into());
        }
        shell.key(editor, 224, true, clipboard)?;
        shell.key(editor, 29, true, clipboard)?;
        shell.key(editor, 224, false, clipboard)?;
        editor.action("history.undo", json!({}))?;
        editor.action(
            "editor.canvas",
            json!({"zoom":0.85,"pan":[36,48],"selection":[from.node]}),
        )?;
        shell.rebuild(editor)?;
        shell.key(editor, 9, true, clipboard)?;
        self.interactions
            .push("UI search added node; sockets connected; keyboard undo restored graph");
        Ok(())
    }
    fn load_large_graph(&self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        let mut text =
            String::from("material Navigation : surface budget 1 {\nlet v0 : float = 0.001;\n");
        for i in 1..250 {
            text.push_str(&format!(
                "let v{i} : float = 0.001;\nlet a{i} : float = add({}, v{i});\n",
                if i == 1 {
                    "v0".into()
                } else {
                    format!("a{}", i - 1)
                }
            ));
        }
        text.push_str("output roughness = a249;\n}");
        let result=editor.action("editor.resource.create",json!({"path":format!("samples/graph-500-{}.graph",editor.host.store().revision()),"domain":"material","source":text}))?;
        let doc = result["doc"].as_str().ok_or("benchmark material")?;
        let graph = ara_graph::Graph::from_document(
            editor
                .host
                .store()
                .workspace()
                .doc(doc)
                .ok_or("large graph")?,
        )
        .map_err(|d| d.to_string())?;
        if graph.nodes.len() != 500 {
            return Err(format!("expected 500 nodes, got {}", graph.nodes.len()).into());
        }
        let mut layout = graph.editor;
        for (i, id) in graph.nodes.keys().enumerate() {
            layout
                .positions
                .insert(id.clone(), [(i % 20) as f32 * 260., (i / 20) as f32 * 164.]);
        }
        editor.apply_to(
            doc,
            vec![json!({"cmd":"graph.editor.set","args":{"editor":layout}})],
            "Lay out 500-node navigation fixture",
        )?;
        editor.action(
            "editor.workspace",
            json!({"resource":doc,"asset":"","item":""}),
        )?;
        editor.action("editor.canvas", json!({"zoom":0.5,"pan":[24,24]}))?;
        shell.rebuild(editor)?;
        Ok(())
    }
}
