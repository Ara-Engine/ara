//! The requested interaction route, using native input helpers and the generated MCP adapter.
use super::*;
use ara_ui_text::EditEvent;
pub(super) struct Verification {
    step: u32,
    output: PathBuf,
    entity: Option<String>,
    txn: Option<String>,
    captures: Vec<String>,
    samples: Vec<Value>,
    started: Instant,
}
impl Verification {
    pub fn new(output: &std::path::Path) -> Self {
        Self {
            step: 0,
            output: output.into(),
            entity: None,
            txn: None,
            captures: Vec::new(),
            samples: Vec::new(),
            started: Instant::now(),
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
            2 => {
                let id = "edit-Transform-position-0";
                shell.click(id, &mut clipboard)?;
                shell.event(InputEvent::Edit(EditEvent::SelectAll), &mut clipboard)?;
                shell.event(
                    InputEvent::Edit(EditEvent::Insert("1.5m".into())),
                    &mut clipboard,
                )?;
                shell.dispatch(editor)?;
                shell.commit_fields(editor)?;
                check_x(editor, self.entity.as_deref(), 1.5)?;
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
                self.screenshot("agents-review", editor, shell, gpu, ui_renderer, renderer)?;
                shell.click(&format!("revert-{txn}"), &mut clipboard)?;
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
                self.screenshot("ara-dark", editor, shell, gpu, ui_renderer, renderer)?;
                editor.action("editor.theme", json!({"light":true}))?;
                shell.rebuild(editor)?;
            }
            9 => {
                self.screenshot("ara-light", editor, shell, gpu, ui_renderer, renderer)?;
                editor.action("editor.theme", json!({"light":false}))?;
                shell.rebuild(editor)?;
            }
            10 => {
                std::fs::create_dir_all(&self.output)?;
                gpu_cpu(&self.output, renderer)?;
                editor.action("editor.palette", json!({"open":true}))?;
                editor.action("editor.search", json!({"panel":"palette","query":"fr"}))?;
                shell.rebuild(editor)?;
                self.screenshot("palette-dark", editor, shell, gpu, ui_renderer, renderer)?;
                editor.action("editor.theme", json!({"light":true}))?;
                shell.rebuild(editor)?;
                self.screenshot("palette-light", editor, shell, gpu, ui_renderer, renderer)?;
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
                std::fs::write(
                    self.output.join("interactions.json"),
                    ara_json::to_canonical_string(
                        &json!({"verified":true,"steps":self.samples,"totalMs":self.started.elapsed().as_secs_f64()*1000.,"flow":["hierarchy selection","unit-aware inspector position","coalesced simulated gizmo","keyboard undo","agent batch via generated MCP","Agents panel turn","Revert via UI button","Escape cancels gesture","MCP viewport/editor/layout capture jobs completed"]}),
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
    pub(super) fn screenshot(
        &self,
        name: &str,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        ui_renderer: &mut UiRenderer,
        renderer: &mut Renderer,
    ) -> ara_ui::Result<()> {
        shell.settle_transitions();
        shell.frame()?;
        let mut scene_view = super::viewport::SceneView::default();
        let view = editor.view()?;
        renderer.update_geometry(view.scene(editor.host.root(), &editor.state, None)?)?;
        if let Some(rect) = shell.viewport() {
            renderer.resize(extent_scaled(rect, gpu.dpi()))?;
            let capture = renderer.capture(&CaptureRequest {
                camera: editor.state.camera.spec(),
                size: renderer.size(),
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 1,
                tick: 0,
                seed: 7,
                marks: Some(100),
            })?;
            if name == "ara-dark" {
                std::fs::create_dir_all(&self.output)?;
                capture.write(&self.output.join("viewport.png"))?;
            }
            scene_view.render(renderer, editor, &view, 64)?;
            scene_view.attach(gpu, renderer, editor.state.quality)?;
            let changed = review_tints(editor);
            gpu.overlays(
                rect,
                overlay::ENTITY_IMAGE,
                overlay::build(&view, &editor.state, rect, renderer, &changed)?,
            );
        }
        crate::shell::icons::install(ui_renderer, editor.state.light, gpu.dpi())?;
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
            ara_json::to_canonical_string(&shell.ui.inspect()),
        )?;
        // Restore live images before releasing the temporary fidelity capture target.
        super::attach(gpu, renderer)?;
        gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
        scene_view.destroy(renderer)?;
        Ok(())
    }
}
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
