//! SDL event loop and same-device native UI/scene composition.
mod capture;
mod controller;
mod game;
mod journey;
mod panels_verify;
mod responsiveness;
mod setup;
mod ux_verify;
mod verify;
mod viewport;
mod windows;
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
/// How long after the last input progressive Scene frames resume.
const INTERACTION_PAUSE: Duration = Duration::from_millis(250);
/// Open the SDL3 editor, host the project authority and serve authenticated EP/MCP.
pub fn run(options: Options) -> ara_ui::Result<()> {
    if options.headless {
        return headless(options);
    }
    if options.verify_panels || options.verify_ux {
        panels_verify::fixtures(&options.project)?;
    }
    // Bounded runs resolve an isolated ARA_CONFIG_DIR (`Options::isolate_profile`);
    // tests and cargo runs are refused the owner's profile.
    let user_settings = crate::settings::user_settings_path()?;
    let mut editor = Editor::open_with(&options.project, Some(user_settings))?;
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
    // Bounded runs (captures, verification routes) never put a window on the desktop:
    // they render offscreen and read the UI target back.
    let offscreen = options.offscreen();
    let window = if offscreen {
        Window::hidden(&title, options.size)?
    } else {
        Window::new(&title, options.size)?
    };
    let mut native = Native::attach(&title)?;
    if options.verify || options.verify_panels || options.verify_ux || options.screenshot {
        native.verification_extent(options.size)?;
    }
    let mut clipboard = OsClipboard;
    crate::native::frame::style(window.raw(), editor.state.light);
    let main_id = window.id();
    let dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
    let physical = window.size()?;
    let logical = physical.map(|v| v as f32 / dpi);
    if options.verify || options.verify_panels || options.verify_ux {
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
    detect_quality(&mut editor, &renderer);
    // SAFETY: Renderer/device are declared before the UI adapter, and survive its destruction.
    let mut gpu = unsafe { NativeUi::new(renderer.gpu().context())? };
    shell.device_info =
        serde_json::to_string_pretty(&renderer.gpu().context().info).unwrap_or_default();
    let mut floats = windows::Floats::default();
    floats.hidden = offscreen;
    let mut game = game::GameView::default();
    gpu.set_dpi(dpi)?;
    let mut ui_renderer = UiRenderer::default();
    crate::shell::icons::install(&mut ui_renderer, dpi)?;
    // Icons are white masks tinted by CSS: only a DPI change (or a verification
    // capture at another scale, `None`) re-rasterizes them; themes never do.
    let mut icon_dpi = Some(dpi);
    // Placeholders for the GPU-fed images: the Scene view and the Game view.
    for id in [overlay::viewport_image(), game::texture()] {
        ui_renderer.image(
            id,
            Image {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        )?;
    }
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
    let mut ux_verification = if options.verify_ux {
        Some(ux_verify::UxVerification::new(
            &mut editor,
            &options.output,
        )?)
    } else {
        None
    };
    let mut probe = options
        .verify_responsiveness
        .then(|| responsiveness::Probe::new(&options.output));
    if probe.is_some() {
        // Measure from the default layout and theme, whatever an earlier run left.
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.theme", json!({"light": false}))?;
    }
    let mut journey = if options.verify_journey {
        Some(journey::Journey::new(&mut editor, &options.output)?)
    } else {
        None
    };
    if panel_verification.is_some()
        || ux_verification.is_some()
        || journey.is_some()
        || probe.is_some()
    {
        shell.rebuild(&mut editor)?;
    }

    let (import_send, imports) = std::sync::mpsc::channel::<PathBuf>();
    if let Ok(mut guard) = IMPORTS.lock() {
        *guard = Some(import_send);
    }
    let result = (|| -> ara_ui::Result<()> {
        let mut last_revision = editor.host.store().revision();
        let mut last_view = editor.view_revision;
        let mut last_scene = editor.scene_revision;
        let mut last_panels = editor.panels_revision;
        let mut last_review = editor.review_revision;
        let mut last_extraction = editor.extraction_revision;
        let mut preview_present = false;
        let mut last_physical = physical;
        let mut last_dpi = dpi;
        let mut dirty = true;
        let mut scene_dirty = true;
        let mut frames = 0u64;
        let mut exit_requested = false;
        let mut controller = controller::Controller::default();
        let mut idle_samples = Vec::new();
        let mut render_samples = Vec::new();
        let mut interaction_samples = Vec::new();
        let mut camera_samples = Vec::new();
        let mut wall_samples = Vec::new();
        let mut last_ui = 0;
        let frame_budget = Duration::from_secs_f64(1. / 144.);
        // The last pointer, key or wheel input: progressive Scene frames wait while the
        // user is working (see below).
        let mut last_input: Option<Instant> = None;
        let mut previous = Instant::now();
        loop {
            if let Some(probe) = &mut probe {
                probe.begin(&mut shell);
            }
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
            let mut events = Window::poll_tagged()?;
            let main_dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
            let mut inject_ms = 0.;
            if let Some(probe) = &mut probe {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                let at = Instant::now();
                probe.inject(&mut editor, &mut shell, scale, &mut events)?;
                inject_ms = at.elapsed().as_secs_f64() * 1000.;
            }
            let interaction_start = (!events.is_empty()).then(Instant::now);
            if events.iter().any(|(_, event)| {
                matches!(
                    event,
                    Event::MouseMotion { .. }
                        | Event::MouseButton { .. }
                        | Event::Wheel(_)
                        | Event::Key { .. }
                )
            }) {
                last_input = Some(Instant::now());
            }
            let routed_at = Instant::now();
            let mut quit = false;
            for (sdl, event) in events {
                let routed = route_event(
                    sdl,
                    event,
                    main_id,
                    main_dpi,
                    &window,
                    &mut floats,
                    &mut controller,
                    &mut editor,
                    &mut shell,
                    &renderer,
                    &view,
                    &mut clipboard,
                );
                match routed {
                    Ok((ui, scene, close)) => {
                        dirty |= ui;
                        scene_dirty |= scene;
                        quit |= close;
                    }
                    Err(error) => {
                        shell.leave();
                        editor.report_failure(&*error);
                        dirty = true;
                    }
                }
            }
            drop(input_scope);
            if quit || exit_requested || editor.host.stopping() {
                drop(profile_scope);
                if profile_frame.is_some() {
                    let _ = editor
                        .host
                        .profiler()
                        .end_frame(Default::default(), Default::default());
                }
                game.close(&mut editor, &mut gpu)?;
                break;
            }
            // IME text goes to the window with keyboard focus.
            let focus = crate::native::keyboard_focus();
            let focused_float = floats
                .windows
                .iter()
                .find(|f| f.sdl == focus)
                .map(|f| f.id.clone());
            if let Some(id) = &focused_float {
                shell.enter(id);
            }
            match native.input(&mut shell.ui, &mut shell.input) {
                Ok(changed) => dirty |= changed,
                Err(error) => editor.report_failure(&*error),
            }
            shell.leave();
            match shell.dispatch(&mut editor) {
                Ok(changed) => dirty |= changed,
                Err(error) => editor.report_failure(&*error),
            }
            for id in floats
                .windows
                .iter()
                .map(|f| f.id.clone())
                .collect::<Vec<_>>()
            {
                if shell.enter(&id) {
                    let result = shell.dispatch(&mut editor);
                    shell.leave();
                    match result {
                        Ok(changed) => dirty |= changed,
                        Err(error) => editor.report_failure(&*error),
                    }
                }
            }
            let input_ms = routed_at.elapsed().as_secs_f64() * 1000.;
            if let Some(request) = shell.window_request.take() {
                match request {
                    crate::shell::WindowRequest::Fullscreen => window.toggle_fullscreen()?,
                    crate::shell::WindowRequest::Raise(id) => {
                        if let Some(f) = floats.windows.iter().find(|f| f.id == id) {
                            f.window.raise();
                        }
                    }
                    crate::shell::WindowRequest::Gather => floats.gather(&window)?,
                    crate::shell::WindowRequest::Import => import_dialog(&window, &editor),
                    crate::shell::WindowRequest::Quit => exit_requested = true,
                }
                dirty = true;
            }
            for path in imports.try_iter() {
                if let Err(error) = import_file(&mut editor, &path) {
                    editor.report_failure(&*error);
                }
                dirty = true;
            }
            if let Some(text) = shell.take_clipboard() {
                use ara_ui_text::Clipboard;
                let _ = clipboard.write(&text);
            }
            shell.main_rect = window.rect()?;
            match controller.fly(&mut editor, &shell, dt) {
                Ok(changed) => {
                    dirty |= changed;
                    scene_dirty |= changed;
                }
                Err(error) => editor.report_failure(&*error),
            }
            if (verification.as_ref().is_some_and(|v| v.finished())
                || ux_verification.as_ref().is_some_and(|v| v.finished()))
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
            let tick_at = Instant::now();
            server.pump(&mut editor);
            editor.tick();
            let tick_ms = tick_at.elapsed().as_secs_f64() * 1000.;
            // The Game view: its size (physical pixels) is where embedded Play starts and
            // what it resizes to; new frames redraw the UI.
            if let Some(i) = shell.ui.find("game-surface") {
                let r = shell.ui.nodes[i].rect;
                let scale =
                    options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
                let frame = shell
                    .ui
                    .find("game-frame")
                    .map(|f| shell.ui.nodes[f].rect)
                    .filter(|f| f.w > 1. && f.h > 1.)
                    .unwrap_or(r);
                let size = [
                    (frame.w * scale).round() as u32,
                    (frame.h * scale).round() as u32,
                ];
                if size[0] >= 64 && size[1] >= 64 {
                    shell.game_target = size;
                    editor.game_size = size;
                }
            }
            let was_live = shell.game_live;
            let was_fault = shell.game_fault.clone();
            let game_frame =
                game.update(&mut editor, &mut shell, &mut gpu, renderer.gpu().context())?;
            dirty |= game_frame;
            if shell.game_live != was_live || shell.game_fault != was_fault {
                if !shell.game_live {
                    shell.game_captured = false;
                }
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            if shell.run_queued(&mut editor) {
                dirty = true;
            }
            if floats.windows.len() != editor.state.floating.len()
                || floats
                    .windows
                    .iter()
                    .zip(&editor.state.floating)
                    .any(|(a, b)| a.id != b.id)
            {
                dirty |= floats.sync(
                    &mut editor,
                    &mut shell,
                    &window,
                    &renderer,
                    &mut gpu,
                    last_dpi,
                )?;
            } else if frames.is_multiple_of(30) {
                floats.persist_rects(&mut editor, controller.buttons_down())?;
                dirty |= floats.sync(
                    &mut editor,
                    &mut shell,
                    &window,
                    &renderer,
                    &mut gpu,
                    last_dpi,
                )?;
            }
            if shell.poll_build(&mut editor) {
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            let assets_changed = shell.poll_assets();
            for (kind, message) in std::mem::take(&mut shell.pending_reports) {
                editor.report(kind, &message, None);
            }
            if assets_changed {
                shell.rebuild(&mut editor)?;
                dirty = true;
            }
            dirty |= shell.upload_thumbnails(&mut ui_renderer)?;
            let geometry_at = Instant::now();
            let (mut view_ms, mut extract_ms, mut upload_ms) = (0., 0., 0.);
            if editor.host.store().revision() != last_revision {
                last_revision = editor.host.store().revision();
                let at = Instant::now();
                view = editor.view()?;
                view_ms = at.elapsed().as_secs_f64() * 1000.;
                let at = Instant::now();
                let scene =
                    view.scene(editor.host.root(), &editor.state, editor.gesture.as_ref())?;
                extract_ms = at.elapsed().as_secs_f64() * 1000.;
                let at = Instant::now();
                renderer.update_geometry(scene)?;
                upload_ms = at.elapsed().as_secs_f64() * 1000.;
                shell.rebuild_if_stale(&mut editor)?;
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
            let geometry_ms = geometry_at.elapsed().as_secs_f64() * 1000.;
            if editor.view_revision != last_view {
                last_view = editor.view_revision;
                dirty = true;
            }
            if editor.scene_revision != last_scene {
                last_scene = editor.scene_revision;
                scene_dirty = true;
            }
            if editor.panels_revision != last_panels {
                last_panels = editor.panels_revision;
                dirty |= shell.rebuild_if_stale(&mut editor)?;
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
                    icon_dpi = None;
                }
                dirty |= changed;
                scene_dirty |= dirty;
            }
            if let Some(route) = &mut journey {
                let changed = route.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                )?;
                dirty |= changed;
                scene_dirty |= changed;
            }
            if let Some(suite) = &mut ux_verification {
                let was_finished = suite.finished();
                let changed = suite.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                    &floats,
                )?;
                if changed {
                    icon_dpi = None;
                }
                dirty |= changed;
                scene_dirty |= changed;
                if !was_finished && suite.finished() {
                    idle_samples.clear();
                    wall_samples.clear();
                    render_samples.clear();
                    interaction_samples.clear();
                }
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
            let dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
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
            let ui_at = Instant::now();
            let work = shell.frame()?;
            let ui_ms = ui_at.elapsed().as_secs_f64() * 1000.;
            let (mut render_ms, mut present_ms, mut presented) = (0., 0., false);
            let (mut scene_ms, mut icons_ms) = (0., 0.);
            let graph_paint = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
            drop(ui_scope);
            dirty |= work.paint_nodes > 0 || work.layout_passes > 0 || work.restyled > 0;
            // Progressive Scene frames (fidelity accumulation, newly published assets)
            // block this thread for their GPU time (27-50 ms on large scenes), so they
            // wait while the user is working: no pointer, key or wheel input for
            // INTERACTION_PAUSE. A click or hover then never queues behind one. Scene
            // changes made by input (camera, gizmo drags, edits) still render at once.
            if shell.viewport().is_some()
                && scene_view.needs_frame(editor.state.quality)
                && (scene_dirty || last_input.is_none_or(|t| t.elapsed() >= INTERACTION_PAUSE))
            {
                dirty = true;
                scene_dirty = true;
            }
            if dirty {
                let _render_scope = profile_frame.map(|_| ara_profile::scope("editor.render"));
                profile_gpu = shell.viewport().is_some() && scene_dirty;
                let render_start = Instant::now();
                let scene_at = Instant::now();
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
                    let mut changed = editor.agent_tints();
                    for id in view.ghosts().keys() {
                        changed.insert(crate::world::render_id(id), "destructive-foreground");
                    }
                    let mut overlays = overlay::build(
                        &view,
                        &editor.state,
                        rect,
                        &renderer,
                        &changed,
                        controller.gizmo_state(),
                    )?;
                    if rect.contains(shell.pointer[0], shell.pointer[1])
                        && !editor.state.palette
                        && shell.menu.is_none()
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
                scene_ms = scene_at.elapsed().as_secs_f64() * 1000.;
                if icon_dpi != Some(dpi) {
                    let icons_at = Instant::now();
                    crate::shell::icons::install(&mut ui_renderer, dpi)?;
                    icon_dpi = Some(dpi);
                    icons_ms = icons_at.elapsed().as_secs_f64() * 1000.;
                }
                gpu.select_target(0);
                gpu.set_dpi(dpi)?;
                let render_at = Instant::now();
                ui_renderer.render(
                    &mut gpu,
                    &shell.ui.display,
                    &shell.text.assets,
                    shell.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                render_ms = render_at.elapsed().as_secs_f64() * 1000.;
                let graph_gpu = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
                let (image, size) = gpu.output()?;
                let present_at = Instant::now();
                // SAFETY: UI submission is complete, image is CopySource and all native owners are live.
                unsafe {
                    present.display(renderer.gpu().context(), image, size)?;
                }
                present_ms = present_at.elapsed().as_secs_f64() * 1000.;
                presented = true;
                let graph_present = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
                if work.paint_nodes > 0 || work.layout_passes > 0 || shell.version != last_ui {
                    if focused_float.is_none() {
                        native.update(&shell.ui, &shell.input, dpi)?;
                    }
                    last_ui = shell.version;
                }
                for float in &mut floats.windows {
                    float.dirty = true;
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
            // Floating windows render after the main window, each into its own target.
            for float in &mut floats.windows {
                let work = shell.frame_surface(&float.id)?;
                if !(float.dirty || work.paint_nodes > 0 || work.layout_passes > 0) {
                    continue;
                }
                let float_dpi = options.dpi.unwrap_or_else(|| float.window.display_scale())
                    * windows::ui_scale(&editor);
                let Some(surface) = shell.surface(&float.id) else {
                    continue;
                };
                gpu.select_target(float.key);
                gpu.set_dpi(float_dpi)?;
                ui_renderer.render(
                    &mut gpu,
                    &surface.ui.display,
                    &shell.text.assets,
                    surface.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                let (image, size) = gpu.output()?;
                // SAFETY: UI submission is complete, image is CopySource and both native owners are live.
                unsafe {
                    float
                        .present
                        .display(renderer.gpu().context(), image, size)?;
                }
                if focused_float.as_deref() == Some(float.id.as_str()) {
                    native.text_input(
                        float.window.raw(),
                        &surface.ui,
                        &surface.input,
                        float_dpi,
                    )?;
                }
                gpu.select_target(0);
                gpu.set_dpi(last_dpi)?;
                float.dirty = false;
            }
            if let Ok(task) = editor.captures.try_recv() {
                let prepared =
                    capture::prepare(&task, &mut renderer, &mut gpu, &shell, &editor, &scene_view);
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
            if let Some(probe) = &mut probe {
                let sample = responsiveness::Sample {
                    cpu_ms: frame_start.elapsed().as_secs_f64() * 1000.,
                    inject_ms,
                    input_ms,
                    ui_ms,
                    tick_ms,
                    geometry_ms,
                    view_ms,
                    extract_ms,
                    upload_ms,
                    scene_ms,
                    icons_ms,
                    render_ms,
                    gpu_ms: gpu.take_gpu_wait().as_secs_f64() * 1000.,
                    present_ms,
                    presented,
                    game_frame,
                    ..Default::default()
                };
                probe.end(&editor, &mut shell, sample);
            }
            frames += 1;
            if probe.as_ref().is_some_and(responsiveness::Probe::finished) {
                let device = serde_json::to_value(&renderer.gpu().context().info)?;
                if let Some(probe) = &probe {
                    probe.write(&device)?;
                }
                game.close(&mut editor, &mut gpu)?;
                break;
            }
            if options.frames.is_some_and(|limit| frames >= limit)
                || verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 5000)
                || panel_verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 1500)
                || ux_verification.as_ref().is_some_and(|v| {
                    v.finished() && idle_samples.len() >= 1500 && camera_samples.len() >= 240
                })
                || journey.as_ref().is_some_and(journey::Journey::finished)
            {
                game.close(&mut editor, &mut gpu)?;
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
        if ux_verification.as_ref().is_some_and(|v| !v.finished()) {
            return Err("UX verification did not finish".into());
        }
        if journey.as_ref().is_some_and(|v| !v.finished()) {
            return Err("the journey route did not finish".into());
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
        if options.verify || options.verify_panels || options.verify_ux {
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
    floats.destroy(&renderer);
    result
}
/// Exercise the editor's document extraction and production renderer without SDL.
fn headless(options: Options) -> ara_ui::Result<()> {
    std::fs::create_dir_all(&options.output)?;
    let mut editor = Editor::open_with(
        &options.project,
        Some(crate::settings::user_settings_path()?),
    )?;
    setup::prepare(&mut editor, &options)?;
    // Headless has no editor tick loop to publish completed imports. This capture
    // job owns one asset-ready boundary; SceneView itself remains read-only.
    // The editor owner can fold this block into its shared asset-ready boundary.
    let _published_assets = ara_play_host::PublishedAssets::for_project(editor.host.root())
        .map_err(anyhow::Error::msg)?;
    let worker = ara_play_host::PlayConfig::for_project(editor.host.root()).import_worker;
    ara_play_host::package_scene_with_progress(editor.host.root(), &worker, |progress| {
        eprintln!(
            "Scene capture cooking: {}/{} complete ({:.1}s elapsed, {:.1}s since completion)",
            progress.completed,
            progress.total,
            progress.elapsed.as_secs_f64(),
            progress.stalled.as_secs_f64()
        );
        Ok(())
    })
    .map_err(anyhow::Error::msg)?;
    let view = editor.view()?;
    let mut renderer = Renderer::new(
        view.scene(editor.host.root(), &editor.state, None)?,
        options.size,
        &[],
        editor.host.root().join(".ara/gpu"),
    )?;
    detect_quality(&mut editor, &renderer);
    let mut scene_view = viewport::SceneView::default();
    scene_view.render(&mut renderer, &editor, &view, 64)?;
    scene_view
        .capture(
            &mut renderer,
            &CaptureRequest {
                camera: editor.state.camera.spec(),
                size: options.size,
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 0.0015,
                settle_frames: 4,
                tick: 0,
                seed: 0,
                marks: Some(16),
            },
        )?
        .write(&options.output.join("editor-scene.png"))?;
    scene_view.destroy(&mut renderer)?;
    editor.host.flush();
    renderer.gpu().assert_clean()?;
    Ok(())
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

/// Route one tagged SDL event: main-window events go straight to the controller,
/// floating-window events run with that window's surface swapped in, and tab drags
/// track the pointer across every window on the desktop.
#[allow(clippy::too_many_arguments)]
fn route_event(
    sdl: u32,
    event: Event,
    main_id: u32,
    dpi: f32,
    window: &Window,
    floats: &mut windows::Floats,
    controller: &mut controller::Controller,
    editor: &mut Editor,
    shell: &mut Shell,
    renderer: &Renderer,
    view: &crate::world::DocumentView,
    clipboard: &mut OsClipboard,
) -> ara_ui::Result<(bool, bool, bool)> {
    if let Some(handled) = game_input(&event, editor, shell, dpi) {
        return Ok((handled, false, false));
    }
    // Tab drags follow the desktop pointer: preview in whichever window is under it.
    if shell.dragged_tab().is_some() {
        match &event {
            Event::MouseMotion { .. } => {
                let global = Window::global_mouse();
                track_tab_drag(global, dpi, window, floats, shell)?;
                for f in &mut floats.windows {
                    f.dirty = true;
                }
                return Ok((true, false, false));
            }
            // A tab pressed but not yet dragged (no drop preview: under 6 px of
            // movement) is an ordinary click: its release goes to the controller.
            Event::MouseButton {
                button: 1,
                pressed: false,
            } if !shell.tab_dragging() => shell.drag = None,
            Event::MouseButton {
                button: 1,
                pressed: false,
            } => {
                let global = Window::global_mouse();
                let rect = [(global[0] - 60.) as i32, (global[1] - 14.) as i32, 420, 560];
                shell.leave();
                let dropped = shell.finish_tab_drag(editor, Some(rect));
                controller.release(1);
                shell.event(InputEvent::Cancel, clipboard)?;
                dropped?;
                return Ok((true, false, false));
            }
            _ => {}
        }
    }
    if sdl == 0 || sdl == main_id {
        return controller.event(
            crate::native::logical_event_for(window.raw(), event, dpi),
            editor,
            shell,
            renderer,
            view,
            clipboard,
        );
    }
    let Some(float) = floats.by_sdl(sdl) else {
        return Ok((false, false, false));
    };
    float.dirty = true;
    let id = float.id.clone();
    let float_dpi = float.window.display_scale() * windows::ui_scale(editor);
    match event {
        Event::Quit => {
            // Closing a floating window closes its panels; Window reopens them docked.
            let panels = editor
                .state
                .floating
                .iter()
                .find(|f| f.id == id)
                .map(|f| panels_of(&f.root))
                .unwrap_or_default();
            for panel in panels {
                editor.action("editor.layout.close", json!({"panel": panel}))?;
            }
            Ok((true, false, false))
        }
        Event::Resize(size) => {
            if size.contains(&0) {
                return Ok((false, false, false));
            }
            float.physical = size;
            // SAFETY: Wait before replacing a swapchain referenced by presentation.
            unsafe {
                renderer.gpu().context().device.device_wait_idle()?;
                float.present.destroy(renderer.gpu().context());
                float.present = Present::new(renderer.gpu().context(), &float.window)?;
            }
            shell.resize_surface(&id, size.map(|v| v as f32 / float_dpi));
            shell.rebuild(editor)?;
            Ok((true, false, false))
        }
        event => {
            let event = crate::native::logical_event_for(float.window.raw(), event, float_dpi);
            if !shell.enter(&id) {
                return Ok((false, false, false));
            }
            let result = controller.event(event, editor, shell, renderer, view, clipboard);
            shell.leave();
            result
        }
    }
}
/// Point the tab-drag preview at the window under a desktop point.
fn track_tab_drag(
    global: [f32; 2],
    dpi: f32,
    window: &Window,
    floats: &windows::Floats,
    shell: &mut Shell,
) -> ara_ui::Result<()> {
    if let Some(float) = floats.at(global) {
        let scale =
            crate::native::points_to_logical(float.window.raw(), float.window.display_scale());
        let local = [
            (global[0] - float.rect[0] as f32) * scale,
            (global[1] - float.rect[1] as f32) * scale,
        ];
        if shell.enter(&float.id) {
            shell.pointer = local;
            shell.drag_over();
            shell.leave();
        }
        return Ok(());
    }
    let m = window.rect()?;
    let inside = global[0] >= m[0] as f32
        && global[1] >= m[1] as f32
        && global[0] < (m[0] + m[2]) as f32
        && global[1] < (m[1] + m[3]) as f32;
    shell.leave();
    if inside {
        let scale = crate::native::points_to_logical(window.raw(), dpi);
        shell.pointer = [
            (global[0] - m[0] as f32) * scale,
            (global[1] - m[1] as f32) * scale,
        ];
        shell.drag_over();
    } else {
        shell.drag_outside();
    }
    Ok(())
}
fn panels_of(node: &ara_ui::docking::DockNode) -> Vec<String> {
    match node {
        ara_ui::docking::DockNode::Area { tabs, .. } => tabs.clone(),
        ara_ui::docking::DockNode::Split { first, second, .. } => {
            let mut out = panels_of(first);
            out.extend(panels_of(second));
            out
        }
    }
}
/// Files chosen in the OS import dialog; SDL may call back on another thread.
static IMPORTS: std::sync::Mutex<Option<std::sync::mpsc::Sender<PathBuf>>> =
    std::sync::Mutex::new(None);
/// Show the OS open-file dialog; chosen files arrive on the import channel.
fn import_dialog(window: &Window, editor: &Editor) {
    unsafe extern "C" fn chosen(
        _: *mut std::ffi::c_void,
        files: *const *const std::ffi::c_char,
        _: std::ffi::c_int,
    ) {
        if files.is_null() {
            return;
        }
        // SAFETY: SDL passes a null-terminated array of UTF-8 paths valid for this call.
        unsafe {
            let mut at = files;
            while !(*at).is_null() {
                let path =
                    PathBuf::from(std::ffi::CStr::from_ptr(*at).to_string_lossy().into_owned());
                if let Ok(guard) = IMPORTS.lock()
                    && let Some(send) = guard.as_ref()
                {
                    let _ = send.send(path);
                }
                at = at.add(1);
            }
        }
    }
    let folder = std::ffi::CString::new(
        editor
            .host
            .root()
            .join(&editor.state.asset_folder)
            .to_string_lossy()
            .into_owned(),
    )
    .ok();
    // SAFETY: The window is live; SDL copies the location and calls back on the main thread.
    unsafe {
        sdl3_sys::dialog::SDL_ShowOpenFileDialog(
            Some(chosen),
            std::ptr::null_mut(),
            window.raw(),
            std::ptr::null(),
            0,
            folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
            true,
        );
    }
}
/// Copy an external file into the current asset folder and import it.
fn import_file(editor: &mut Editor, source: &std::path::Path) -> ara_ui::Result<()> {
    let name = source.file_name().ok_or("import path has no file name")?;
    let folder = editor.state.asset_folder.clone();
    let relative = if folder.is_empty() {
        name.to_string_lossy().into_owned()
    } else {
        format!("{folder}/{}", name.to_string_lossy())
    };
    let destination = editor.host.root().join(&relative);
    if destination != source {
        std::fs::copy(source, &destination)?;
    }
    editor.action("editor.asset.import", json!({"path": relative}))?;
    Ok(())
}
/// While the Game view holds the keyboard and mouse, raw input goes to the shown Play
/// client (the player maps it exactly like its own window). Esc, or pressing outside
/// the view, gives them back; F5-F8 (Play, Pause, Step, Stop) stay with the editor.
/// `None` lets the editor handle the event; `Some(redraw)` consumed it.
fn game_input(event: &Event, editor: &mut Editor, shell: &mut Shell, dpi: f32) -> Option<bool> {
    use ara_play_host::embed::DeviceEvent;
    if !shell.game_captured {
        return None;
    }
    let release = |editor: &mut Editor, shell: &mut Shell| {
        shell.game_captured = false;
        if let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces) {
            surfaces.device(world, shell.game_client, vec![DeviceEvent::FocusLost]);
        }
        let _ = shell.rebuild(editor);
    };
    let events = match *event {
        Event::Key {
            scancode: 41,
            pressed: true,
            ..
        } => {
            release(editor, shell);
            return Some(true);
        }
        // F5-F8: the editor's Play controls.
        Event::Key {
            scancode: 62..=65, ..
        } => return None,
        Event::Key { repeat: true, .. } => return Some(false),
        Event::Key {
            scancode, pressed, ..
        } => vec![DeviceEvent::Key {
            scancode,
            down: pressed,
        }],
        Event::MouseButton { button, pressed } => {
            let inside = shell.ui.find("game-frame").is_some_and(|i| {
                shell.ui.nodes[i]
                    .rect
                    .contains(shell.pointer[0], shell.pointer[1])
            });
            if pressed && !inside {
                release(editor, shell);
                return None;
            }
            vec![DeviceEvent::MouseButton {
                button,
                down: pressed,
            }]
        }
        Event::MouseMotion { position, delta } => {
            shell.pointer = [position[0] / dpi, position[1] / dpi];
            vec![DeviceEvent::MouseMotion {
                dx: delta[0],
                dy: delta[1],
            }]
        }
        _ => return None,
    };
    if let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces) {
        surfaces.device(world, shell.game_client, events);
    }
    Some(false)
}

/// The GPU's automatic preset: what Auto (and an Auto project default) resolve to in
/// the Scene view's menus, status bar and `quality` gating.
fn detect_quality(editor: &mut Editor, renderer: &Renderer) {
    use ara_render_fidelity::presets::{Capabilities, QualityPreset};
    editor.state.detected_quality = Some(QualityPreset::detect(&Capabilities::of(renderer.gpu())));
    editor.state.apply_settings();
}
