//! The requested interaction route, using native input helpers and the generated MCP adapter.
use super::*;
use ara_ui_text::EditEvent;
pub(super) struct Verification {
    step: u32,
    output: PathBuf,
    entity: Option<String>,
    txn: Option<String>,
    captures: Vec<String>,
    /// Scene viewport captures of the typed-draft check: before, draft, committed.
    scene: Vec<image::RgbaImage>,
    samples: Vec<Value>,
    started: Instant,
    /// Gizmo hover/drag state drawn into captures (the panels route's Scene states).
    gizmo: overlay::GizmoState,
}
impl Verification {
    pub fn new(output: &std::path::Path) -> Self {
        Self {
            step: 0,
            output: output.into(),
            entity: None,
            txn: None,
            captures: Vec::new(),
            scene: Vec::new(),
            samples: Vec::new(),
            started: Instant::now(),
            gizmo: Default::default(),
        }
    }
    pub fn finished(&self) -> bool {
        self.step >= 14
    }
    pub fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui_renderer: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if self.finished() {
            return Ok(false);
        }
        let start = Instant::now();
        let mut clipboard = OsClipboard;
        match self.step {
            0 => {
                self.entity = crate::world::resolve(editor.host.store(), &editor.state.doc)
                    .map_err(|d| format!("{d:?}"))?
                    .iter()
                    .find(|(_, e)| {
                        e.record.get("name").and_then(Value::as_str) == Some("Salvage crate")
                    })
                    .map(|(id, _)| id.clone());
                if self.entity.is_none() {
                    return Err("--verify needs the sample scene created by --sample".into());
                }
            }
            1 => {
                let entity = self.entity.as_deref().ok_or("verification entity")?;
                shell.click_entity(entity, &mut clipboard)?;
                shell.dispatch(editor)?;
                if editor.state.selection.first().map(String::as_str) != Some(entity) {
                    return Err("hierarchy did not select the expected entity".into());
                }
            }
            // A typed draft previews in the Scene before it commits. Over three frames
            // (the loop projects drafts and applies commits between steps): capture,
            // type; capture the draft, commit; capture the commit. The draft frame
            // matches the committed one, and the crate moved from the first.
            2 if self.scene.is_empty() => {
                let before = self.scene_image(editor, shell, gpu, renderer, scene_view)?;
                self.scene.push(before);
                let id = "edit-Transform-position-0";
                // Engine components (MeshRenderer) can push Transform below the fold.
                shell.reveal_field("Transform.position.x")?;
                shell.click(id, &mut clipboard)?;
                shell.event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)?;
                shell.event(
                    InputEvent::Edit(EditEvent::Insert("1.5m".into())),
                    &mut clipboard,
                )?;
                shell.dispatch(editor)?;
                return Ok(true);
            }
            2 if self.scene.len() == 1 => {
                let draft = self.scene_image(editor, shell, gpu, renderer, scene_view)?;
                self.scene.push(draft);
                shell.commit_fields(editor)?;
                check_x(editor, self.entity.as_deref(), 1.5)?;
                return Ok(true);
            }
            2 => {
                let committed = self.scene_image(editor, shell, gpu, renderer, scene_view)?;
                self.draft_preview(committed)?;
            }
            3 => {
                let before = editor.host.store().history().len();
                shell.input.blur(&mut shell.ui);
                editor.begin_gesture()?;
                for amount in [0.1, 0.3, 0.8] {
                    if let Some(gesture) = &mut editor.gesture {
                        gesture.update(Vec3::new(amount, 0., 0.));
                    }
                }
                editor.finish_gesture(false)?;
                if editor.host.store().history().len() != before + 1 {
                    return Err("gizmo did not coalesce to one transaction".into());
                }
                check_x(editor, self.entity.as_deref(), 2.3)?;
                shell.rebuild(editor)?;
            }
            4 => {
                shell.key(editor, 224, true, &mut clipboard)?;
                shell.key(editor, 29, true, &mut clipboard)?;
                shell.key(editor, 224, false, &mut clipboard)?;
                check_x(editor, self.entity.as_deref(), 1.5)?;
            }
            5 => {
                let mut mcp = ara_mcp::Adapter::new();
                let begun=mcp.call_tool(editor,"ara_session",json!({"action":"begin","client":"editor-verification","author":"agent:verification","label":"Reposition salvage"}).as_object().cloned().ok_or("MCP argument object")?).map_err(|e|e.to_string())?;
                let session = begun["structuredContent"]["session"]
                    .as_str()
                    .ok_or("MCP session result")?;
                let arguments=json!({"session":session,"doc":editor.state.doc,"why":"Make room for the salvage route","commands":[{"cmd":"transform.set","args":{"entities":[self.entity],"position":[3.,0.6,0.]}}]}).as_object().cloned().ok_or("MCP argument object")?;
                let result = mcp
                    .call_tool(editor, "ara_apply", arguments)
                    .map_err(|e| e.to_string())?;
                if result["isError"] == true {
                    return Err(format!("MCP batch failed: {result}").into());
                }
                self.txn = result["structuredContent"]["txn"]
                    .as_str()
                    .map(str::to_owned);
                if self.txn.is_none() {
                    return Err("MCP batch did not return a transaction".into());
                }
                editor.action("editor.focus", json!({"panel":"agents"}))?;
                shell.rebuild(editor)?;
            }
            6 => {
                let txn = self.txn.as_deref().ok_or("verification transaction")?;
                if shell.ui.find(&format!("turn-{txn}")).is_none() {
                    return Err("Agents panel did not show the MCP turn".into());
                }
                self.screenshot(
                    "agents-review",
                    editor,
                    shell,
                    gpu,
                    ui_renderer,
                    renderer,
                    scene_view,
                )?;
                shell.click(&format!("turn-{txn}-undo"), &mut clipboard)?;
                shell.dispatch(editor)?;
                check_x(editor, self.entity.as_deref(), 1.5)?;
            }
            7 => {
                editor.begin_gesture()?;
                if let Some(gesture) = &mut editor.gesture {
                    gesture.update(Vec3::new(90., 0., 0.));
                }
                let before = editor.host.store().revision();
                editor.finish_gesture(true)?;
                if editor.host.store().revision() != before {
                    return Err("Escape/cancel mutated authoring state".into());
                }
                editor.action("editor.focus", json!({"panel":"assets"}))?;
                shell.rebuild(editor)?;
            }
            8 => {
                self.screenshot(
                    "ara-dark",
                    editor,
                    shell,
                    gpu,
                    ui_renderer,
                    renderer,
                    scene_view,
                )?;
                editor.action("editor.theme", json!({"light":true}))?;
                shell.rebuild(editor)?;
            }
            9 => {
                self.screenshot(
                    "ara-light",
                    editor,
                    shell,
                    gpu,
                    ui_renderer,
                    renderer,
                    scene_view,
                )?;
                editor.action("editor.theme", json!({"light":false}))?;
                shell.rebuild(editor)?;
            }
            10 => {
                std::fs::create_dir_all(&self.output)?;
                gpu_cpu(&self.output, renderer)?;
                editor.action("editor.palette", json!({"open":true}))?;
                editor.action("editor.search", json!({"panel":"palette","query":"fr"}))?;
                shell.rebuild(editor)?;
                self.screenshot(
                    "palette-dark",
                    editor,
                    shell,
                    gpu,
                    ui_renderer,
                    renderer,
                    scene_view,
                )?;
                editor.action("editor.theme", json!({"light":true}))?;
                shell.rebuild(editor)?;
                self.screenshot(
                    "palette-light",
                    editor,
                    shell,
                    gpu,
                    ui_renderer,
                    renderer,
                    scene_view,
                )?;
                editor.action("editor.theme", json!({"light":false}))?;
                editor.action("editor.palette", json!({"open":false}))?;
                shell.rebuild(editor)?;
            }
            11 => {
                let mut mcp = ara_mcp::Adapter::new();
                for target in ["viewport", "editor", "layout"] {
                    let arguments=json!({"session":editor.session,"target":target,"path":format!(".ara/captures/verification-{target}.png")}).as_object().cloned().ok_or("capture argument object")?;
                    let result = mcp
                        .call_tool(editor, "ara_capture", arguments)
                        .map_err(|e| e.to_string())?;
                    self.captures.push(
                        result["structuredContent"]["job"]
                            .as_str()
                            .ok_or("capture job result")?
                            .to_owned(),
                    );
                }
            }
            12 => {
                for job in &self.captures {
                    let status = editor.action("jobs.status", json!({"job":job}))?;
                    match status["status"].as_str() {
                        Some("running") => {
                            if self.started.elapsed() > Duration::from_secs(30) {
                                return Err("MCP captures timed out".into());
                            }
                            return Ok(false);
                        }
                        Some("done") => {
                            let path = status["result"]["path"]
                                .as_str()
                                .ok_or("capture output path")?;
                            if !std::path::Path::new(path).is_file() {
                                return Err("capture completed without an output artifact".into());
                            }
                        }
                        _ => return Err(format!("capture failed: {status}").into()),
                    }
                }
            }
            13 => {
                // The warnings and errors the run left (the status bar counts them).
                let problems = editor.action(
                    "logs",
                    json!({"group": false, "limit": 500, "since": 0, "severity": "warning"}),
                )?;
                std::fs::write(
                    self.output.join("problems.json"),
                    ara_json::to_canonical_string(&problems),
                )?;
                std::fs::write(
                    self.output.join("interactions.json"),
                    ara_json::to_canonical_string(
                        &json!({"verified":true,"steps":self.samples,"totalMs":self.started.elapsed().as_secs_f64()*1000.,"flow":["hierarchy selection","unit-aware inspector position","typed draft previews in the Scene","coalesced simulated gizmo","keyboard undo","agent batch via generated MCP","Agents panel turn","Revert via UI button","Escape cancels gesture","MCP viewport/editor/layout capture jobs completed"]}),
                    ),
                )?;
            }
            _ => {}
        }
        self.samples
            .push(json!({"step":self.step,"cpuMs":start.elapsed().as_secs_f64()*1000.}));
        self.step += 1;
        Ok(true)
    }
    /// The Scene viewport as a reproducible capture, showing what the live view shows
    /// (a typed draft's projection included).
    fn scene_image(
        &self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<image::RgbaImage> {
        shell.frame()?;
        let view = editor.view()?;
        let rect = shell.viewport().ok_or("the Scene viewport is not shown")?;
        renderer.resize(extent_scaled(rect, gpu.dpi()))?;
        scene_view.render_capture(renderer, editor, &view, 64)?;
        let capture = scene_view.capture(
            renderer,
            &CaptureRequest {
                camera: editor.state.camera.spec(),
                size: renderer.size(),
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 1,
                tick: 0,
                seed: 7,
                marks: None,
            },
        )?;
        scene_view.attach(gpu, renderer, editor.state.quality)?;
        Ok(capture.image)
    }
    /// Check the typed-draft captures (before, draft, then `committed`) and keep them
    /// as evidence: the draft must show what the commit shows, unlike the frame before.
    fn draft_preview(&mut self, committed: image::RgbaImage) -> ara_ui::Result<()> {
        let [before, draft] = std::mem::take(&mut self.scene)
            .try_into()
            .map_err(|_| "the typed-draft check needs two captures")?;
        std::fs::create_dir_all(&self.output)?;
        for (name, image) in [
            ("before", &before),
            ("draft", &draft),
            ("committed", &committed),
        ] {
            image.save(self.output.join(format!("viewport-{name}.png")))?;
        }
        let same = ara_render::capture::compare(&committed, &draft, DRAFT_TOLERANCE)?;
        let moved = ara_render::capture::compare(&committed, &before, DRAFT_TOLERANCE)?;
        std::fs::write(
            self.output.join("draft-preview.json"),
            ara_json::to_canonical_string(
                &json!({"draftVsCommitted":same.score,"beforeVsCommitted":moved.score,"tolerance":DRAFT_TOLERANCE}),
            ),
        )?;
        if !same.passed {
            return Err(format!(
                "the typed draft's Scene differs from its commit (1-SSIM {})",
                same.score
            )
            .into());
        }
        if moved.passed {
            return Err(format!(
                "the Scene did not change with the edit (1-SSIM {})",
                moved.score
            )
            .into());
        }
        Ok(())
    }
    /// Draw the gizmo with this hover/drag state in later captures.
    pub(super) fn with_gizmo(mut self, gizmo: overlay::GizmoState) -> Self {
        self.gizmo = gizmo;
        self
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn screenshot(
        &self,
        name: &str,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui_renderer: &mut UiRenderer,
        renderer: &mut Renderer,
        scene_view: &mut super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        // Pending saves land first and the shell rebuilds after them, so captures
        // show a settled save state (the status bar, the unsaved-scene dot): a save
        // that finished since the last rebuild still reads "Saving…" until then.
        editor.host.flush();
        shell.rebuild(editor)?;
        shell.settle_transitions();
        shell.frame()?;
        let view = editor.view()?;
        // A native Scene extracts its own content; the legacy renderer takes the
        // committed scene, without the gesture.
        if !super::scene_sync::native(editor, renderer) {
            renderer.update_geometry(view.scene(
                editor.host.root(),
                &editor.state,
                None,
                &mut editor.models,
            )?)?;
        }
        if let Some(rect) = shell.viewport() {
            renderer.resize(extent_scaled(rect, gpu.dpi()))?;
            scene_view.render_capture(renderer, editor, &view, 64)?;
            let capture = scene_view.capture(
                renderer,
                &CaptureRequest {
                    camera: editor.state.camera.spec(),
                    size: renderer.size(),
                    mode: ViewMode::Lit,
                    deterministic: true,
                    exposure: 0.0015,
                    settle_frames: 1,
                    tick: 0,
                    seed: 7,
                    marks: Some(100),
                },
            )?;
            if name == "ara-dark" {
                std::fs::create_dir_all(&self.output)?;
                capture.write(&self.output.join("viewport.png"))?;
            }
            scene_view.attach(gpu, renderer, editor.state.quality)?;
            let changed = editor.agent_tints();
            let overlays = overlay::build(
                &view,
                &editor.state,
                rect,
                renderer,
                &changed,
                self.gizmo,
                &mut shell.outlines,
                editor.host.store().revision(),
            )?;
            shell.outlines.upload(ui_renderer, gpu)?;
            gpu.overlays(rect, overlay::ENTITY_IMAGE, overlays);
        }
        crate::shell::icons::install(ui_renderer, gpu.dpi())?;
        ui_renderer.render(
            gpu,
            &shell.ui.display,
            &shell.text.assets,
            shell.ui.size.map(|v| v.round() as u32),
            TextMode::Raster,
        )?;
        std::fs::create_dir_all(&self.output)?;
        gpu.pixels()?
            .save(self.output.join(format!("{name}.png")))?;
        std::fs::write(
            self.output.join(format!("{name}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        // The capture settled the live view; retain that same completed attachment.
        scene_view.attach(gpu, renderer, editor.state.quality)?;
        gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
        Ok(())
    }
}
/// The 1-SSIM within which two Scene captures count as the same image.
const DRAFT_TOLERANCE: f64 = 0.005;
fn check_x(editor: &Editor, entity: Option<&str>, expected: f32) -> ara_ui::Result<()> {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|d| format!("{d:?}"))?;
    let record = &resolved[entity.ok_or("verification entity")?].record;
    let position = crate::world::gesture::transform_value(record, "position");
    if (position.x - expected).abs() > 0.0001 {
        return Err(format!("expected X={expected}, got {}", position.x).into());
    }
    Ok(())
}
fn gpu_cpu(output: &std::path::Path, renderer: &Renderer) -> ara_ui::Result<()> {
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let mut ui = ara_ui::Ui::editor(
        "<ui id='comparison'><panel id='clip' style='width:220px; height:90px; overflow:hidden; border-radius:18px; background:var(--card)'><panel id='inner' style='width:180px; height:80px; overflow:hidden; border-radius:12px; background:var(--accent-surface)'><text id='raster-label' text='Ara raster / clipped' style='font-size:18px' /><button id='sample' text='Premultiplied alpha' /></panel></panel></ui>",
        ara_ui::theme::EDITOR_CSS,
        [300., 140.],
    )?;
    ui.frame(&mut text)?;
    let scalable = ui.display.commands.iter().find_map(|draw| {
        if let ara_ui::paint::Draw::GlyphRun {
            font,
            glyphs,
            color,
            ..
        } = draw
        {
            Some(ara_ui::paint::Draw::ScalableGlyphRun {
                node: 900,
                font: *font,
                size: 14.,
                origin: [10., 100.],
                glyphs: glyphs.clone(),
                color: *color,
            })
        } else {
            None
        }
    });
    if let Some(draw) = scalable {
        ui.display.commands.push(draw);
    }
    ui.display.commands.push(ara_ui::paint::Draw::Line {
        node: 901,
        from: [12., 130.],
        to: [270., 105.],
        width: 2.,
        color: ara_ui::style::Color(76, 130, 230, 255),
    });
    for mode in [TextMode::Raster, TextMode::Msdf] {
        let mut draw = UiRenderer::default();
        let mut cpu = ara_ui_render::CpuGpu::default();
        draw.render(&mut cpu, &ui.display, &text.assets, [300, 140], mode)?;
        draw.device_lost();
        // SAFETY: The borrowed renderer/device outlives this local adapter; queue use remains serialized.
        let mut gpu = unsafe { NativeUi::new(renderer.gpu().context())? };
        draw.render(&mut gpu, &ui.display, &text.assets, [300, 140], mode)?;
        let image = gpu.pixels()?;
        let name = match mode {
            TextMode::Raster => "raster",
            TextMode::Msdf => "msdf",
        };
        image.save(output.join(format!("gpu-{name}.png")))?;
        cpu.pixmap()?
            .save_png(output.join(format!("cpu-{name}.png")))?;
        let reference = image::RgbaImage::from_raw(300, 140, cpu.pixmap()?.data().to_vec())
            .ok_or("CPU reference extent")?;
        let comparison = ara_render::capture::compare(&reference, &image, 0.01)?;
        comparison
            .diff
            .save(output.join(format!("gpu-cpu-{name}.diff.png")))?;
        std::fs::write(
            output.join(format!("gpu-cpu-{name}.json")),
            ara_json::to_canonical_string(
                &json!({"score":comparison.score,"passed":comparison.passed,"tolerance":0.01}),
            ),
        )?;
        if !comparison.passed {
            return Err(format!("GPU/CPU {name} comparison failed").into());
        }
    }
    Ok(())
}
