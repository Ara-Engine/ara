//! SDL event loop and same-device native UI/scene composition.
mod capture;
mod controller;
mod verify;
use crate::{
    Editor,
    gpu::NativeUi,
    native::{Native, OsClipboard},
    shell::{Shell, overlay},
    transport::Server,
    world::EditWorldView,
};
use ara_platform::{Event, Present, Window};
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
};
use ara_ui::input::InputEvent;
use ara_ui_render::{
    atlas::TextMode,
    gpu::{Image, Renderer as UiRenderer},
};
use glam::Vec3;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
/// Editor launch settings; bounded verification runs use the same production event loop.
pub struct Options {
    /// Project directory containing ara.project.json.
    pub project: PathBuf,
    /// Stop after this many event-loop frames; None keeps the editor open.
    pub frames: Option<u64>,
    /// Run the scripted native/protocol verification route and write evidence.
    pub verify: bool,
    /// Populate an empty scene using one typed sample-content batch.
    pub sample: bool,
    /// Screenshots and measurement directory.
    pub output: PathBuf,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            project: ".".into(),
            frames: None,
            verify: false,
            sample: false,
            output: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence"),
        }
    }
}
/// Open the SDL3 editor, host the project authority and serve authenticated EP/MCP.
pub fn run(options: Options) -> ara_ui::Result<()> {
    let mut editor = Editor::open(&options.project)?;
    if options.sample {
        sample(&mut editor)?;
    }
    let mut server = Server::start(editor.host.root())?;
    eprintln!(
        "Ara editor authority listening on 127.0.0.1:{}",
        server.port
    );
    let title = format!(
        "Ara | {}",
        editor
            .host
            .root()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let window = Window::new(&title, [1440, 900])?;
    let mut native = Native::attach(&title)?;
    let mut clipboard = OsClipboard;
    let dpi = native.dpi();
    let physical = window.size()?;
    let logical = physical.map(|v| v as f32 / dpi);
    let mut shell = Shell::new(&mut editor, logical)?;
    let mut view = editor.view()?;
    let viewport = shell.viewport().unwrap_or(ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 800.,
        h: 600.,
    });
    let mut renderer = Renderer::new(
        view.scene(editor.host.root(), &editor.state, None)?,
        extent(viewport),
        &window.extensions()?,
        editor.host.root().join(".ara/gpu"),
    )?;
    // SAFETY: Renderer/device are declared before the UI adapter, and survive its destruction.
    let mut gpu = unsafe { NativeUi::new(renderer.gpu().context())? };
    gpu.set_dpi(dpi)?;
    let mut ui_renderer = UiRenderer::default();
    ui_renderer.image(
        overlay::viewport_image(),
        Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        },
    )?;
    // SAFETY: Both native owners are retained until explicit idle/destroy after the loop, including errors.
    let mut present = unsafe { Present::new(renderer.gpu().context(), &window)? };
    let mut verification = options
        .verify
        .then(|| verify::Verification::new(&options.output));
    let result = (|| -> ara_ui::Result<()> {
        let mut last_revision = editor.host.store().revision();
        let mut last_view = editor.view_revision;
        let mut last_panels = editor.panels_revision;
        let mut last_review = editor.review_revision;
        let mut last_extraction = editor.extraction_revision;
        let mut preview_present = false;
        let mut last_physical = physical;
        let mut last_dpi = dpi;
        let mut dirty = true;
        let mut scene_dirty = true;
        let mut frames = 0u64;
        let mut controller = controller::Controller::default();
        let mut idle_samples = Vec::new();
        let mut render_samples = Vec::new();
        let mut interaction_samples = Vec::new();
        let mut camera_samples = Vec::new();
        let mut wall_samples = Vec::new();
        let mut last_ui = 0;
        let frame_budget = Duration::from_secs_f64(1. / 144.);
        let mut previous = Instant::now();
        loop {
            let frame_start = Instant::now();
            let interval = frame_start.duration_since(previous).as_secs_f64() * 1000.;
            let mut camera_start = None;
            let dt = frame_start.duration_since(previous).as_secs_f32().min(0.05);
            previous = frame_start;
            let events = window.poll()?;
            let interaction_start = (!events.is_empty()).then(Instant::now);
            let mut quit = false;
            for event in events {
                match controller.event(
                    event,
                    &mut editor,
                    &mut shell,
                    &renderer,
                    &view,
                    &mut clipboard,
                ) {
                    Ok((ui, scene, close)) => {
                        dirty |= ui;
                        scene_dirty |= scene;
                        quit |= close;
                    }
                    Err(error) => {
                        editor.report_error(&error.to_string());
                        dirty = true;
                    }
                }
            }
            if quit || editor.host.stopping() {
                break;
            }
            match native.input(&mut shell.ui, &mut shell.input) {
                Ok(changed) => dirty |= changed,
                Err(error) => editor.report_error(&error.to_string()),
            }
            match shell.dispatch(&mut editor) {
                Ok(changed) => dirty |= changed,
                Err(error) => editor.report_error(&error.to_string()),
            }
            match controller.fly(&mut editor, &shell, dt) {
                Ok(changed) => {
                    dirty |= changed;
                    scene_dirty |= changed;
                }
                Err(error) => editor.report_error(&error.to_string()),
            }
            if verification.as_ref().is_some_and(|v| v.finished())
                && frames >= 32
                && camera_samples.len() < 240
            {
                camera_start = Some(Instant::now());
                let mut camera = editor.state.camera;
                camera.yaw += 0.002;
                set_camera(&mut editor, &camera)?;
                dirty = true;
                scene_dirty = true;
            }
            server.pump(&mut editor);
            editor.tick();
            if shell.poll_assets() {
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            if editor.host.store().revision() != last_revision {
                last_revision = editor.host.store().revision();
                view = editor.view()?;
                renderer.update_geometry(view.scene(
                    editor.host.root(),
                    &editor.state,
                    editor.gesture.as_ref(),
                )?)?;
                shell.rebuild(&mut editor)?;
                dirty = true;
                scene_dirty = true;
                preview_present = editor.gesture.is_some();
            }
            if editor.extraction_revision != last_extraction {
                last_extraction = editor.extraction_revision;
                view = editor.view()?;
                renderer.update_geometry(view.scene(
                    editor.host.root(),
                    &editor.state,
                    editor.gesture.as_ref(),
                )?)?;
                dirty = true;
                scene_dirty = true;
            }
            if editor.review_revision != last_review {
                last_review = editor.review_revision;
                let next = editor.view()?;
                if next.ghosts() != view.ghosts() {
                    renderer.update_geometry(next.scene(
                        editor.host.root(),
                        &editor.state,
                        editor.gesture.as_ref(),
                    )?)?;
                }
                view = next;
                dirty = true;
                scene_dirty = true;
            }
            if editor.view_revision != last_view {
                last_view = editor.view_revision;
                dirty = true;
                scene_dirty = true;
            }
            if editor.panels_revision != last_panels {
                last_panels = editor.panels_revision;
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            if let Some(verification) = &mut verification {
                dirty |= verification.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                )?;
                scene_dirty |= dirty;
            }
            let physical = window.size()?;
            if physical.contains(&0) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let dpi = native.dpi();
            if physical != last_physical || dpi != last_dpi {
                // SAFETY: Wait before destroying a swapchain referenced by the presentation queue.
                unsafe {
                    renderer.gpu().context().device.device_wait_idle()?;
                    present.destroy(renderer.gpu().context());
                    present = Present::new(renderer.gpu().context(), &window)?;
                }
                last_physical = physical;
                last_dpi = dpi;
                gpu.set_dpi(dpi)?;
                shell.ui.size = physical.map(|v| v as f32 / dpi);
                shell.rebuild(&mut editor)?;
                dirty = true;
                scene_dirty = true;
            }
            let work = shell.frame()?;
            dirty |= work.paint_nodes > 0 || work.layout_passes > 0 || work.restyled > 0;
            if dirty {
                let render_start = Instant::now();
                if let Some(rect) = shell.viewport() {
                    renderer.resize(extent(rect))?;
                    if scene_dirty {
                        if editor.gesture.is_some() || preview_present {
                            renderer.update_geometry(view.scene(
                                editor.host.root(),
                                &editor.state,
                                editor.gesture.as_ref(),
                            )?)?;
                            preview_present = editor.gesture.is_some();
                        }
                        renderer.render(&editor.state.camera.spec(), 0.0015)?;
                    }
                    attach(&mut gpu, &renderer)?;
                    let mut changed = review_tints(&editor);
                    for id in view.ghosts().keys() {
                        changed.insert(crate::world::render_id(id), "destructive-foreground");
                    }
                    gpu.overlays(
                        rect,
                        overlay::ENTITY_IMAGE,
                        overlay::build(&view, &editor.state, rect, &renderer, &changed)?,
                    );
                } else {
                    gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
                }
                ui_renderer.render(
                    &mut gpu,
                    &shell.ui.display,
                    &shell.text.assets,
                    shell.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                let (image, size) = gpu.output()?;
                // SAFETY: UI submission is complete, image is CopySource and all native owners are live.
                unsafe {
                    present.display(renderer.gpu().context(), image, size)?;
                }
                if work.paint_nodes > 0 || work.layout_passes > 0 || shell.version != last_ui {
                    native.update(&shell.ui, &shell.input)?;
                    last_ui = shell.version;
                }
                render_samples.push(render_start.elapsed().as_secs_f64() * 1000.);
                if let Some(start) = camera_start {
                    camera_samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
                dirty = false;
                scene_dirty = false;
                if let Some(start) = interaction_start {
                    interaction_samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
            } else {
                if frames >= 32 {
                    idle_samples.push(frame_start.elapsed().as_secs_f64() * 1000.);
                    wall_samples.push(interval);
                }
            }
            if let Ok(task) = editor.captures.try_recv() {
                let prepared = capture::prepare(&task, &mut renderer, &mut gpu, &shell, &editor);
                let completed = editor.completed.clone();
                match prepared {
                    Ok(work) => {
                        std::thread::spawn(move || {
                            let result = work
                                .write()
                                .map_err(|e| vec![crate::invalid(e.to_string())]);
                            let _ = completed.send(crate::capture::CaptureCompletion {
                                job: task.job,
                                result,
                            });
                        });
                    }
                    Err(error) => {
                        let _ = completed.send(crate::capture::CaptureCompletion {
                            job: task.job,
                            result: Err(vec![crate::invalid(error.to_string())]),
                        });
                    }
                }
            }
            frames += 1;
            if options.frames.is_some_and(|limit| frames >= limit)
                || verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 5000)
            {
                break;
            }
            if let Some(remaining) = frame_budget.checked_sub(frame_start.elapsed()) {
                native.wait(remaining);
            }
        }
        if verification.as_ref().is_some_and(|v| !v.finished()) {
            return Err("native verification did not complete within the frame budget".into());
        }
        editor.host.flush();
        renderer.gpu().assert_clean()?;
        if options.verify {
            std::fs::create_dir_all(&options.output)?;
            let timings = json!({"targetHz":144,"warmCameraToPresentMs":statistics(&camera_samples),"idleFrameIntervalMs":statistics(&wall_samples),"idleCpuMs":statistics(&idle_samples),"dirtyRenderAndPresentMs":statistics(&render_samples),"nativeInputToPresentMs":statistics(&interaction_samples),"idleSamplesMs":idle_samples,"renderSamplesMs":render_samples,"warmCameraSamplesMs":camera_samples,"idleIntervalSamplesMs":wall_samples,"interactionSamplesMs":interaction_samples,"device":renderer.gpu().context().info,"note":"CPU active time excludes idle sleep; native presentation and current renderer serialize. Scripted timings are recorded separately."});
            std::fs::write(
                options.output.join("timings.json"),
                ara_json::to_canonical_string(&timings),
            )?;
        }
        Ok(())
    })();
    // SAFETY: Complete all presentation/UI submissions before native swapchain/surface destruction.
    unsafe {
        renderer.gpu().context().device.device_wait_idle()?;
        present.destroy(renderer.gpu().context());
    }
    result
}
fn extent(rect: ara_ui::paint::Box2) -> [u32; 2] {
    [rect.w.round().max(1.) as u32, rect.h.round().max(1.) as u32]
}
fn set_camera(
    editor: &mut Editor,
    camera: &crate::state::Camera,
) -> Result<(), crate::EditorError> {
    editor.action("editor.camera",json!({"target":camera.target.to_array(),"yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance}))?;
    Ok(())
}
fn attach(gpu: &mut NativeUi, renderer: &Renderer) -> anyhow::Result<()> {
    // SAFETY: Both views belong to the shared live renderer, stay valid through UI submission, and finish CopySource.
    unsafe {
        gpu.external_image(
            overlay::viewport_image(),
            renderer.gpu().native_image(renderer.output())?,
            renderer.gpu().image_view(renderer.output())?,
        )?;
        gpu.external_image(
            overlay::ENTITY_IMAGE,
            renderer.gpu().native_image(renderer.entity_channel())?,
            renderer.gpu().image_view(renderer.entity_channel())?,
        )?;
    }
    Ok(())
}
fn review_tints(editor: &Editor) -> std::collections::BTreeMap<u64, &'static str> {
    editor.review_tints()
}

fn statistics(samples: &[f64]) -> Value {
    if samples.is_empty() {
        return json!({"samples":0});
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
    json!({"samples":samples.len(),"p50":percentile(0.5),"p95":percentile(0.95),"p99":percentile(0.99),"max":sorted.last()})
}
fn sample(editor: &mut Editor) -> ara_ui::Result<()> {
    let empty = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .is_some_and(|d| d.records().is_empty());
    if !empty {
        return Ok(());
    }
    let mut commands = Vec::new();
    for (name, position, scale, tint) in [
        (
            "Outpost floor",
            [0., -0.25, 0.],
            [12., 0.5, 10.],
            [0.18, 0.20, 0.23, 1.],
        ),
        (
            "Salvage crate",
            [0., 0.6, 0.],
            [1.2, 1.2, 1.2],
            [0.12, 0.28, 0.42, 1.],
        ),
        (
            "Bulkhead",
            [-3., 1.5, -2.],
            [0.3, 3., 6.],
            [0.25, 0.28, 0.32, 1.],
        ),
        (
            "Signal tower",
            [2.5, 1.5, -2.],
            [0.8, 3., 0.8],
            [0.4, 0.3, 0.14, 1.],
        ),
    ] {
        commands.push(json!({"cmd":"entity.create","args":{"name":name,"components":{"Transform":{"position":position,"scale":scale},"MeshRenderer":{"tint":tint}}}}));
    }
    editor.apply(commands, "Create sample Outpost scene", None)?;
    editor.host.flush();
    Ok(())
}
