//! SDL event loop and same-device native UI/scene composition.
mod capture;
mod controller;
mod panels_verify;
mod setup;
mod verify;
mod viewport;
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
pub use setup::Options;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
/// Open the SDL3 editor, host the project authority and serve authenticated EP/MCP.
pub fn run(options: Options) -> ara_ui::Result<()> {
    if options.verify_panels {
        panels_verify::fixtures(&options.project)?;
    }
    let mut editor = Editor::open(&options.project)?;
    setup::prepare(&mut editor, &options)?;
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
    let window = Window::new(&title, options.size)?;
    let mut native = Native::attach(&title)?;
    if options.verify || options.verify_panels || options.screenshot {
        native.verification_extent(options.size)?;
    }
    let mut clipboard = OsClipboard;
    let dpi = options.dpi.unwrap_or_else(|| native.dpi());
    let physical = window.size()?;
    let logical = physical.map(|v| v as f32 / dpi);
    if options.verify || options.verify_panels {
        eprintln!("Verification extent {physical:?}; logical {logical:?}; DPI {dpi}");
    }
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
        extent_scaled(viewport, dpi),
        &window.extensions()?,
        editor.host.root().join(".ara/gpu"),
    )?;
    // SAFETY: Renderer/device are declared before the UI adapter, and survive its destruction.
    let mut gpu = unsafe { NativeUi::new(renderer.gpu().context())? };
    gpu.set_dpi(dpi)?;
    let mut ui_renderer = UiRenderer::default();
    crate::shell::icons::install(&mut ui_renderer, editor.state.light, dpi)?;
    let mut icon_state = (editor.state.light, dpi);
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
    let mut scene_view = viewport::SceneView::default();
    let mut verification = options
        .verify
        .then(|| verify::Verification::new(&options.output));
    let mut panel_verification = if options.verify_panels {
        Some(panels_verify::Verification::new(
            &mut editor,
            &options.output,
            options.size,
        )?)
    } else {
        None
    };
    if panel_verification.is_some() {
        shell.rebuild(&mut editor)?;
    }

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
            let profile_frame = if editor.host.profiler().is_recording() {
                editor.host.profiler().begin_frame().ok()
            } else {
                None
            };
            let profile_scope = profile_frame.map(|_| ara_profile::scope("editor.frame"));
            let input_scope = profile_frame.map(|_| ara_profile::scope("editor.input"));
            let mut profile_draws = 0;
            let mut profile_triangles = 0;
            let mut profile_gpu = false;
            let frame_start = Instant::now();
            let interval = frame_start.duration_since(previous).as_secs_f64() * 1000.;
            let mut camera_start = None;
            let mut graph_start = None;
            let dt = frame_start.duration_since(previous).as_secs_f32().min(0.05);
            previous = frame_start;
            let events = window.poll()?;
            let interaction_start = (!events.is_empty()).then(Instant::now);
            let mut quit = false;
            for event in events {
                match controller.event(
                    native.logical_event(event, options.dpi.unwrap_or_else(|| native.dpi())),
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
            drop(input_scope);
            if quit || editor.host.stopping() {
                drop(profile_scope);
                if profile_frame.is_some() {
                    let _ = editor
                        .host
                        .profiler()
                        .end_frame(Default::default(), Default::default());
                }
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
            let authority_scope = profile_frame.map(|_| ara_profile::scope("editor.authority"));
            server.pump(&mut editor);
            editor.tick();
            if shell.poll_assets() {
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            dirty |= shell.upload_thumbnails(&mut ui_renderer)?;
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
                let changed = verification.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                )?;
                if changed {
                    icon_state = (!editor.state.light, last_dpi);
                }
                dirty |= changed;
                scene_dirty |= dirty;
            }
            if let Some(suite) = &mut panel_verification {
                let was_finished = suite.finished();
                graph_start = suite.benchmarking().then(Instant::now);
                let changed = suite.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                )?;
                dirty |= changed;
                scene_dirty |= changed;
                if !was_finished && suite.finished() {
                    idle_samples.clear();
                    wall_samples.clear();
                }
            }
            drop(authority_scope);
            let physical = window.size()?;
            if physical.contains(&0) {
                drop(profile_scope);
                if profile_frame.is_some() {
                    let _ = editor
                        .host
                        .profiler()
                        .end_frame(Default::default(), Default::default());
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let dpi = options.dpi.unwrap_or_else(|| native.dpi());
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
            if frames > 0 && frames.is_multiple_of(144) {
                shell.frame_timing(
                    1000. / interval.max(0.1),
                    render_samples.last().copied().unwrap_or(0.),
                )?;
            }
            if recording_profiler_visible(&mut editor) && frames.is_multiple_of(12) {
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            let graph_control = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
            shell.sync_canvas(&editor);
            shell.orient_axis(editor.state.camera);
            let ui_scope = profile_frame.map(|_| ara_profile::scope("editor.ui"));
            let work = shell.frame()?;
            let graph_paint = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
            drop(ui_scope);
            dirty |= work.paint_nodes > 0 || work.layout_passes > 0 || work.restyled > 0;
            if shell.viewport().is_some() && scene_view.needs_frame(editor.state.quality) {
                dirty = true;
                scene_dirty = true;
            }
            if dirty {
                let _render_scope = profile_frame.map(|_| ara_profile::scope("editor.render"));
                profile_gpu = shell.viewport().is_some() && scene_dirty;
                let render_start = Instant::now();
                if let Some(rect) = shell.viewport() {
                    renderer.resize(extent_scaled(rect, dpi))?;
                    if scene_dirty {
                        if editor.gesture.is_some() || preview_present {
                            renderer.update_geometry(view.scene(
                                editor.host.root(),
                                &editor.state,
                                editor.gesture.as_ref(),
                            )?)?;
                            preview_present = editor.gesture.is_some();
                        }
                        scene_view.render(&mut renderer, &editor, &view, 1)?;
                    }
                    scene_view.attach(&mut gpu, &renderer, editor.state.quality)?;
                    let mut changed = review_tints(&editor);
                    for id in view.ghosts().keys() {
                        changed.insert(crate::world::render_id(id), "destructive-foreground");
                    }
                    let mut overlays =
                        overlay::build(&view, &editor.state, rect, &renderer, &changed)?;
                    if rect.contains(shell.pointer[0], shell.pointer[1])
                        && !editor.state.palette
                        && !shell.context
                        && shell
                            .ui
                            .hit_test(shell.pointer[0], shell.pointer[1])
                            .is_none()
                    {
                        let c = shell
                            .ui
                            .sheet
                            .color_token("--muted-foreground")
                            .ok_or("hover token")?;
                        overlays.push(crate::gpu::Overlay::Hover {
                            uv: [
                                (shell.pointer[0] - rect.x) / rect.w,
                                (shell.pointer[1] - rect.y) / rect.h,
                            ],
                            color: [
                                f32::from(c.0) / 255.,
                                f32::from(c.1) / 255.,
                                f32::from(c.2) / 255.,
                                0.65,
                            ],
                        });
                    }
                    gpu.overlays(rect, overlay::ENTITY_IMAGE, overlays);
                } else {
                    gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
                }
                if icon_state != (editor.state.light, dpi) {
                    crate::shell::icons::install(&mut ui_renderer, editor.state.light, dpi)?;
                    icon_state = (editor.state.light, dpi);
                }
                ui_renderer.render(
                    &mut gpu,
                    &shell.ui.display,
                    &shell.text.assets,
                    shell.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                let graph_gpu = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
                let (image, size) = gpu.output()?;
                // SAFETY: UI submission is complete, image is CopySource and all native owners are live.
                unsafe {
                    present.display(renderer.gpu().context(), image, size)?;
                }
                let graph_present = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
                if work.paint_nodes > 0 || work.layout_passes > 0 || shell.version != last_ui {
                    native.update(&shell.ui, &shell.input, dpi)?;
                    last_ui = shell.version;
                }
                render_samples.push(render_start.elapsed().as_secs_f64() * 1000.);
                if let Some(start) = camera_start {
                    camera_samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
                if let (Some(start), Some(suite)) = (graph_start, &mut panel_verification) {
                    suite.record(
                        start.elapsed().as_secs_f64() * 1000.,
                        [
                            graph_control.unwrap_or(0.),
                            graph_paint.unwrap_or(0.),
                            graph_gpu.unwrap_or(0.),
                            graph_present.unwrap_or(0.),
                        ],
                    );
                    if suite.benchmarking()
                        && !suite.samples.is_empty()
                        && suite.samples.len().is_multiple_of(200)
                    {
                        eprintln!(
                            "graph cumulative ms: control={graph_control:?} paint={graph_paint:?} gpu={graph_gpu:?} present={graph_present:?} native={:.3}; {} batches / {} instances",
                            start.elapsed().as_secs_f64() * 1000.,
                            ui_renderer.batches.len(),
                            ui_renderer.instances.len()
                        );
                    }
                }
                profile_draws = ui_renderer.batches.len() as u64;
                profile_triangles = ui_renderer.instances.len() as u64 * 2;
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
            drop(profile_scope);
            if let Some(token) = profile_frame {
                if profile_gpu {
                    editor
                        .host
                        .profiler()
                        .gpu_timings(token.id(), renderer.timings());
                    editor
                        .host
                        .profiler()
                        .gpu_timings(token.id(), scene_view.timings());
                }
                editor
                    .host
                    .profiler()
                    .end_frame(
                        ara_profile::Counters {
                            entities: view.entities().len() as u64,
                            draws: profile_draws,
                            triangles: profile_triangles,
                            ..Default::default()
                        },
                        Default::default(),
                    )
                    .map_err(String::from)?;
            }
            frames += 1;
            if options.frames.is_some_and(|limit| frames >= limit)
                || verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 5000)
                || panel_verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 1500)
            {
                break;
            }
            if let Some(remaining) = frame_budget.checked_sub(frame_start.elapsed()) {
                native.wait(remaining)?;
            }
        }
        if verification.as_ref().is_some_and(|v| !v.finished()) {
            return Err("native verification did not complete within the frame budget".into());
        }
        if panel_verification.as_ref().is_some_and(|v| !v.finished()) {
            return Err("native panel verification did not finish".into());
        }
        editor.host.flush();
        if options.screenshot {
            std::fs::create_dir_all(&options.output)?;
            gpu.pixels()?.save(options.output.join("editor.png"))?;
            std::fs::write(
                options.output.join("editor.layout.json"),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
        }
        renderer.gpu().assert_clean()?;
        if options.verify || options.verify_panels {
            std::fs::create_dir_all(&options.output)?;
            let timings = json!({"targetHz":144,"requestedPixels":options.size,"logicalSize":shell.ui.size,"dpi":last_dpi,"graph500PanZoomInputToPresentMs":panel_verification.as_ref().map(|v|statistics(&v.samples)),"warmCameraToPresentMs":statistics(&camera_samples),"idleFrameIntervalMs":statistics(&wall_samples),"idleCpuMs":statistics(&idle_samples),"dirtyRenderAndPresentMs":statistics(&render_samples),"nativeInputToPresentMs":statistics(&interaction_samples),"idleSamplesMs":idle_samples,"renderSamplesMs":render_samples,"warmCameraSamplesMs":camera_samples,"idleIntervalSamplesMs":wall_samples,"interactionSamplesMs":interaction_samples,"device":renderer.gpu().context().info,"note":"CPU active time excludes idle sleep; native presentation and current renderer serialize. Scripted timings are recorded separately."});
            std::fs::write(
                options.output.join("timings.json"),
                ara_json::to_canonical_string(&timings),
            )?;
        }
        Ok(())
    })();
    scene_view.destroy(&mut renderer)?;
    // SAFETY: Complete all presentation/UI submissions before native swapchain/surface destruction.
    unsafe {
        renderer.gpu().context().device.device_wait_idle()?;
        present.destroy(renderer.gpu().context());
    }
    result
}
fn extent_scaled(rect: ara_ui::paint::Box2, dpi: f32) -> [u32; 2] {
    [
        (rect.w * dpi).round().max(1.) as u32,
        (rect.h * dpi).round().max(1.) as u32,
    ]
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

fn recording_profiler_visible(editor: &mut Editor) -> bool {
    fn active(node: &ara_ui::docking::DockNode) -> bool {
        match node {
            ara_ui::docking::DockNode::Area {
                tabs,
                active: selected,
                ..
            } => tabs.get(*selected).is_some_and(|t| t == "profiler"),
            ara_ui::docking::DockNode::Split { first, second, .. } => {
                active(first) || active(second)
            }
        }
    }
    active(&editor.state.layout)
        && editor.profile_current.is_none()
        && editor.host.profiler().is_recording()
}
