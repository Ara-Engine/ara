//! SDL event loop and same-device native UI/scene composition.
mod asset_ops_verify;
mod assets_verify;
mod boundary_verify;
mod capture;
mod chrome_verify;
mod collection_verify;
mod content_drags;
mod controller;
mod cursors;
mod diagnostics_verify;
mod drags;
mod eyedropper;
mod file_picker;
mod game;
#[cfg(test)]
mod game_input_tests;
mod game_preview;
mod hierarchy_verify;
mod history_verify;
mod import_drops;
mod import_verify;
mod input_verify;
mod journey;
mod monkey;
mod monkey_route;
mod navigation_verify;
mod panels_verify;
mod play_verify;
mod prefab_verify;
mod qa;
mod reachability_verify;
mod responsiveness;
mod scene_quality;
mod scene_sync;
mod selection_verify;
mod setup;
mod snap_verify;
mod ux_verify;
mod verify;
mod viewport;
mod window_chrome;
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
pub use setup::{Options, Settle};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
/// How long after the last input Scene work that blocks this thread waits: ray-traced
/// progressive frames, and adopting newly published content (`SceneSync::refresh`).
const INTERACTION_PAUSE: Duration = Duration::from_millis(250);
/// Longest a window frame waits for the previous UI frame before skipping (a stalled
/// present path: minimized, occluded or undisplayed window), so the pump keeps running.
const UI_STALL_WAIT: Duration = Duration::from_millis(100);
/// How long `--verify-ux` waits for its idle-frame samples once its captures, camera
/// samples and drags are done: a scene with live content (effects previewing in the
/// Scene view) never idles, and the route must still finish.
const UX_IDLE_PATIENCE: Duration = Duration::from_secs(20);
/// When the process started: `main` sets it first. Startup timings
/// (`--verify-responsiveness`) count from it.
pub static LAUNCHED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// Open the SDL3 editor, host the project authority and serve authenticated EP/MCP.
pub fn run(mut options: Options) -> ara_ui::Result<()> {
    if options.verify_behaviours {
        content_drags::fixture(&mut options)?;
        hierarchy_verify::prepare(&options)?;
        std::fs::write(
            options.output.join("verification-scope.json"),
            ara_json::to_canonical_string(&json!({
                "schema":"ara.editor.behaviour-scope/1",
                "scope":if options.behaviour_asset_ops {"full"} else {"editor-core"},
                "extendedImportAndAssetOperationsIncluded":options.behaviour_asset_ops,
                "fullAcceptanceRequiresExtendedAssets":true,
                "size":options.size,"dpi":options.dpi,
            })),
        )?;
    }
    if options.headless {
        return headless(options);
    }
    if options.verify_panels || options.verify_ux || options.verify_docking {
        panels_verify::fixtures(&options.project)?;
    }
    // Bounded runs resolve an isolated ARA_CONFIG_DIR (`Options::isolate_profile`);
    // tests and cargo runs are refused the owner's profile.
    let user_settings = crate::settings::user_settings_path()?;
    let host_seed = if options.verify_monkey {
        Some(match options.monkey_replay.as_deref() {
            Some(journal) => monkey::Monkey::journal_seed(journal)?,
            None => options.monkey_seed,
        })
    } else {
        None
    };
    let mut editor = Editor::open_config(
        &options.project,
        Some(user_settings),
        ara_host::HostConfig {
            seed: host_seed,
            ..Default::default()
        },
    )?;
    setup::prepare(&mut editor, &options)?;
    let mut server = Server::start(editor.host.root())?;
    eprintln!(
        "Ara editor authority listening on 127.0.0.1:{}",
        server.port
    );
    // Interactive opens wait in the loading window until the imports settle; bounded
    // runs open at once, except `--verify-loading`, which only captures that window.
    let mut splash = None;
    if options.verify_loading || !options.offscreen() {
        let capture = options
            .verify_loading
            .then(|| (options.output.as_path(), options.dpi.unwrap_or(1.)));
        match crate::loading::run(&mut editor, &mut server, capture)? {
            crate::loading::Outcome::Open(shown) => splash = shown,
            crate::loading::Outcome::Quit => return Ok(()),
        }
    }
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
    // Behind a loading window, the main window appears with its first frame.
    let mut window = if offscreen || splash.is_some() {
        Window::hidden(&title, options.size)?
    } else {
        Window::new(&title, options.size)?
    };
    // Native icons are cosmetic; Wayland may not support them.
    let _ = window.set_icon_png(include_bytes!(
        "../../../../assets/brand/icons/engine/ara-engine-64.png"
    ));
    let mut native = Native::attach(&title)?;
    if options.verify
        || options.verify_panels
        || options.verify_ux
        || options.verify_docking
        || options.verify_behaviours
        || options.screenshot
    {
        native.verification_extent(options.size)?;
    }
    window.enable_custom_chrome()?;
    let mut caption = window_chrome::SnapCaption::new(&window)?;
    let mut clipboard = crate::native::SessionClipboard::new(offscreen);
    crate::native::frame::style(window.raw(), editor.state.light);
    let main_id = window.id();
    let dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
    let physical = window.size()?;
    let logical = physical.map(|v| v as f32 / dpi);
    if options.verify
        || options.verify_panels
        || options.verify_ux
        || options.verify_docking
        || options.verify_behaviours
    {
        eprintln!("Verification extent {physical:?}; logical {logical:?}; DPI {dpi}");
    }
    let mut shell = Shell::new(&mut editor, logical)?;
    shell.offscreen = offscreen;
    if let Some(button) = shell.update_chrome(&mut window, dpi) {
        caption.update(button);
    }
    if options.verify {
        chrome_verify::verify(
            &mut editor,
            &mut shell,
            &mut window,
            &mut caption,
            &options.output,
            dpi,
        )?;
    }
    let mut view = editor.view()?;
    editor.report_missing_sources(&view);
    let viewport = shell.viewport().unwrap_or(ara_ui::paint::Box2 {
        x: 0.,
        y: 0.,
        w: 800.,
        h: 600.,
    });
    // The publication the first extraction sees; SceneSync re-extracts on newer ones.
    let models_at_open = editor.models.revision();
    let render_settings = crate::world::gltf::render_settings(editor.host.store().workspace())?;
    let ray_tracing = render_settings.ray_tracing;
    let mut opening = crate::loading::Status {
        project: crate::shell::project_name(&editor),
        folder: editor.host.root().display().to_string(),
        light: editor.state.light,
        opening: true,
        ..Default::default()
    };
    let Some(mut renderer) = viewport::prepare_renderer(
        view.scene(editor.host.root(), &editor.state, None, &mut editor.models)?,
        extent_scaled(viewport, dpi),
        window.extensions()?,
        editor.host.root().join(".ara/gpu"),
        ray_tracing,
        render_settings.quality.into(),
        |elapsed| {
            server.pump(&mut editor);
            editor.tick();
            opening.elapsed = elapsed.as_secs();
            if splash.is_none() && !offscreen && elapsed >= Duration::from_millis(400) {
                splash = Some(crate::loading::Splash::opening(&opening)?);
            }
            if let Some(splash) = &mut splash {
                return Ok(splash.opening_frame(&opening)?);
            }
            Ok(!window
                .poll()?
                .iter()
                .any(|event| matches!(event, Event::Quit)))
        },
    )?
    else {
        return Ok(());
    };
    // Typed authority commands/imports continued while the renderer was built.
    view = editor.view()?;
    detect_quality(&mut editor, &renderer);
    // SAFETY: Renderer/device are declared before the UI adapter, and survive its destruction.
    let mut gpu = unsafe { NativeUi::new(crate::gpu::ui_device(renderer.gpu()))? };
    shell.device_info = serde_json::to_string_pretty(renderer.gpu().info()).unwrap_or_default();
    let mut floats = windows::Floats::default();
    floats.hidden = offscreen;
    let mut game = game::GameView::default();
    let mut game_preview = game_preview::Preview::default();
    let mut eyedropper_buttons = eyedropper::Buttons::default();
    gpu.set_dpi(dpi)?;
    let mut ui_renderer = UiRenderer::default();
    crate::shell::icons::install(&mut ui_renderer, dpi)?;
    // Icons are white masks tinted by CSS: only a DPI change (or a verification
    // capture at another scale, `None`) re-rasterizes them; themes never do.
    let mut icon_dpi = Some(dpi);
    // Placeholders for the GPU-fed images: the Scene view, the Game view and its
    // camera preview outside Play.
    for id in [
        overlay::viewport_image(),
        game::texture(),
        game_preview::texture(),
    ] {
        ui_renderer.image(
            id,
            Image {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        )?;
    }
    for player in 0..=16 {
        ui_renderer.image(
            game::player_texture(player),
            Image {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        )?;
    }
    // Offscreen runs keep native input windows but render only into GPU textures.
    let mut present = if offscreen {
        None
    } else {
        // SAFETY: Native owners live until the explicit idle/destroy after the loop.
        Some(unsafe { gpu.create_present(crate::gpu::ui_device(renderer.gpu()), &window)? })
    };
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
    // The UX route's drags run after its captures and camera timings.
    let mut drag_check = if options.verify_docking {
        editor.action("editor.preferences", json!({"reduceMotion": true}))?;
        editor.action("editor.layout.reset", json!({}))?;
        shell.rebuild(&mut editor)?;
        Some(drags::DragCheck::new(&options.output, &shell))
    } else {
        None
    };
    let mut assets_verification = if options.verify_assets {
        Some(assets_verify::AssetsVerification::new(
            &mut editor,
            &options.output,
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
    let mut probe = if options.verify_unpresentable {
        Some(responsiveness::Probe::unpresentable_only(&options.output))
    } else {
        options
            .verify_responsiveness
            .then(|| responsiveness::Probe::new(&options.output, options.settle))
    };
    if probe.is_some() {
        // Measure what a release build does: memoized panels are not double-checked.
        shell.memo_check = false;
        // Measure from the default layout and theme, whatever an earlier run left.
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.theme", json!({"light": false}))?;
    }
    let mut scene_quality = options
        .verify_scene_quality
        .then(|| scene_quality::SceneQuality::new(&options.output));
    if scene_quality.is_some() {
        // What a release build shows, from the default layout and theme.
        shell.memo_check = false;
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.theme", json!({"light": false}))?;
    }
    let mut journey = if options.verify_journey {
        Some(journey::Journey::new(&mut editor, &options.output)?)
    } else {
        None
    };
    let mut first_hour = if options.verify_first_hour {
        Some(qa::FirstHour::new(&mut editor, &options.output)?)
    } else {
        None
    };
    let mut monkey = if let Some(journal) = &options.monkey_replay {
        Some(monkey::Monkey::from_journal(
            journal,
            Duration::from_secs(options.monkey_seconds),
            &options.output,
        )?)
    } else if options.verify_monkey {
        Some(monkey::Monkey::new(
            options.monkey_seed,
            Duration::from_secs(options.monkey_seconds),
            options.monkey_steps,
            &options.output,
        )?)
    } else {
        None
    };
    if let (Some(runner), Some(seed)) = (&mut monkey, host_seed) {
        runner.record_host_seed(seed)?;
    }
    let mut content_drags = if options.verify_behaviours {
        Some(content_drags::ContentDrags::new(
            &mut editor,
            &options.output,
        )?)
    } else {
        None
    };
    let mut navigation_check = None;
    let mut import_check = None;
    let mut asset_ops_check = None;
    let mut selection_check = None;
    let mut hierarchy_check = None;
    let mut collection_check = None;
    let mut snapping_check = None;
    let mut input_check = None;
    let mut boundary_check = None;
    let mut history_check = None;
    let mut prefab_check = None;
    let mut diagnostics_check = None;
    let mut reachability_check = None;
    let mut play_check = None;
    let mut monkey_input = monkey::native::Adapter::default();
    if panel_verification.is_some()
        || ux_verification.is_some()
        || journey.is_some()
        || first_hour.is_some()
        || probe.is_some()
        || scene_quality.is_some()
    {
        shell.rebuild(&mut editor)?;
    }

    let (import_send, imports) = std::sync::mpsc::channel::<file_picker::Selection>();
    let mut import_drops = import_drops::ImportDrops::default();
    let result = (|| -> ara_ui::Result<()> {
        let mut last_revision = editor.host.store().revision();
        let mut last_view = editor.view_revision;
        let mut last_scene = editor.scene_revision;
        let mut last_panels = editor.panels_revision;
        let mut last_review = editor.review_revision;
        let mut last_extraction = editor.extraction_revision;
        // Value edits, typed drafts and gizmo drags patch the Scene in place.
        let mut scene_sync =
            scene_sync::SceneSync::new(models_at_open, editor.host.store().revision());
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
        // When `--verify-ux` had everything but its idle samples.
        let mut ux_idle_since: Option<Instant> = None;
        let mut last_ui = 0;
        let frame_budget = Duration::from_secs_f64(1. / 144.);
        // The last pointer, key or wheel input: progressive Scene frames wait while the
        // user is working (see below).
        let mut last_input: Option<Instant> = None;
        // The last frame the Scene camera moved: right after navigation the Scene
        // converges at once instead.
        let mut last_navigation: Option<Instant> = None;
        let mut previous = Instant::now();
        #[cfg(feature = "hotpatch")]
        let mut rust_patches = ara_hotpatch::Hotpatch::connect();
        loop {
            #[cfg(feature = "hotpatch")]
            if rust_patches.frame_boundary() {
                shell.rust_hotpatched(&mut editor)?;
                dirty = true;
            }
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
            // Host waits on the renderer's device so far (timeline, upload idle,
            // present): the probe subtracts this frame's share from its wall time.
            let waits_before = renderer.gpu().host_wait_stats();
            let interval = frame_start.duration_since(previous).as_secs_f64() * 1000.;
            let mut camera_start = None;
            let mut graph_start = None;
            let dt = frame_start.duration_since(previous).as_secs_f32().min(0.05);
            previous = frame_start;
            let mut events = Window::poll_tagged()?;
            let main_dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
            let mut inject_ms = 0.;
            // What this frame's input edits (scripted input included): typed drafts and
            // commits defer the Scene render.
            let edit_before = (
                editor.host.store().revision(),
                shell.drafts().clone(),
                editor.state.camera,
            );
            if let Some(probe) = &mut probe {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                let at = Instant::now();
                probe.inject(&mut editor, &mut shell, scale, &mut events)?;
                inject_ms = at.elapsed().as_secs_f64() * 1000.;
            }
            if let Some(check) = &mut drag_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    &floats,
                    scale,
                    &mut events,
                )?;
            }
            if let Some(check) = &mut content_drags {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                let scene_ready =
                    verification_scene_ready(&renderer, &scene_view, &editor, last_revision);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    &floats,
                    scale,
                    &scene_view,
                    &mut events,
                    scene_ready,
                )?;
            }
            if content_drags
                .as_ref()
                .is_some_and(content_drags::ContentDrags::finished)
                && options.behaviour_asset_ops
                && import_check.is_none()
            {
                import_check = Some(import_verify::ImportCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut import_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    scale,
                    &floats,
                    &mut events,
                )?;
            }
            if import_check
                .as_ref()
                .is_some_and(import_verify::ImportCheck::finished)
                && asset_ops_check.is_none()
            {
                asset_ops_check = Some(asset_ops_verify::AssetOpsCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut asset_ops_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            let preceding_behaviours_finished = if options.behaviour_asset_ops {
                asset_ops_check
                    .as_ref()
                    .is_some_and(asset_ops_verify::AssetOpsCheck::finished)
            } else {
                content_drags
                    .as_ref()
                    .is_some_and(content_drags::ContentDrags::finished)
            };
            if preceding_behaviours_finished && navigation_check.is_none() {
                navigation_check = Some(navigation_verify::NavigationCheck::new(
                    &editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut navigation_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                let scene_ready =
                    verification_scene_ready(&renderer, &scene_view, &editor, last_revision);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    scale,
                    &scene_view,
                    &mut events,
                    scene_ready,
                )?;
            }
            if navigation_check
                .as_ref()
                .is_some_and(navigation_verify::NavigationCheck::finished)
                && selection_check.is_none()
            {
                selection_check = Some(selection_verify::SelectionCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut selection_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    scale,
                    &scene_view,
                    &mut events,
                    &controller,
                )?;
            }
            if selection_check
                .as_ref()
                .is_some_and(selection_verify::SelectionCheck::finished)
                && hierarchy_check.is_none()
            {
                hierarchy_check = Some(hierarchy_verify::HierarchyCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut hierarchy_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if hierarchy_check
                .as_ref()
                .is_some_and(hierarchy_verify::HierarchyCheck::finished)
                && snapping_check.is_none()
            {
                snapping_check = Some(snap_verify::SnapCheck::new(&mut editor, &options.output)?);
            }
            if let Some(check) = &mut snapping_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(
                    &mut editor,
                    &mut shell,
                    &window,
                    &floats,
                    scale,
                    &scene_view,
                    &mut events,
                )?;
            }
            if snapping_check
                .as_ref()
                .is_some_and(snap_verify::SnapCheck::finished)
                && collection_check.is_none()
            {
                collection_check = Some(collection_verify::CollectionCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut collection_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if collection_check
                .as_ref()
                .is_some_and(collection_verify::CollectionCheck::finished)
                && input_check.is_none()
            {
                input_check = Some(input_verify::InputCheck::new(&options.output));
            }
            if let Some(check) = &mut input_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if input_check
                .as_ref()
                .is_some_and(input_verify::InputCheck::finished)
                && boundary_check.is_none()
            {
                boundary_check = Some(boundary_verify::BoundaryCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut boundary_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if boundary_check
                .as_ref()
                .is_some_and(boundary_verify::BoundaryCheck::finished)
                && history_check.is_none()
            {
                history_check = Some(history_verify::HistoryCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut history_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if history_check
                .as_ref()
                .is_some_and(history_verify::HistoryCheck::finished)
                && prefab_check.is_none()
            {
                prefab_check = Some(prefab_verify::PrefabCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut prefab_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if prefab_check
                .as_ref()
                .is_some_and(prefab_verify::PrefabCheck::finished)
                && diagnostics_check.is_none()
            {
                diagnostics_check =
                    Some(diagnostics_verify::DiagnosticsCheck::new(&options.output));
            }
            if let Some(check) = &mut diagnostics_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if diagnostics_check
                .as_ref()
                .is_some_and(diagnostics_verify::DiagnosticsCheck::finished)
                && reachability_check.is_none()
            {
                reachability_check = Some(reachability_verify::ReachabilityCheck::new(
                    &mut editor,
                    &options.output,
                )?);
            }
            if let Some(check) = &mut reachability_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if reachability_check
                .as_ref()
                .is_some_and(reachability_verify::ReachabilityCheck::finished)
                && play_check.is_none()
            {
                play_check = Some(play_verify::PlayCheck::new(&mut editor, &options.output)?);
            }
            if let Some(check) = &mut play_check {
                let scale = crate::native::points_to_logical(window.raw(), main_dpi);
                check.inject(&mut editor, &mut shell, &window, scale, &mut events)?;
            }
            if let Some(runner) = &mut monkey {
                let monkey_at = Instant::now();
                let changed = monkey_route::step(
                    runner,
                    &mut monkey_input,
                    main_dpi,
                    &window,
                    &mut floats,
                    &mut controller,
                    &mut editor,
                    &mut shell,
                    &mut renderer,
                    &gpu,
                    &view,
                    &scene_view,
                    &mut import_drops,
                )?;
                inject_ms += monkey_at.elapsed().as_secs_f64() * 1000.0;
                dirty |= changed;
                scene_dirty |= changed;
                if changed {
                    last_input = Some(Instant::now());
                }
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
            }) || probe.as_ref().is_some_and(|p| p.acted())
            {
                last_input = Some(Instant::now());
            }
            let routed_at = Instant::now();
            let mut quit = false;
            for (sdl, event) in events {
                let event_clipboard: &mut dyn ara_ui_text::Clipboard = if let Some(check) =
                    diagnostics_check.as_mut().filter(|check| !check.finished())
                {
                    check.clipboard()
                } else if let Some(check) = history_check.as_mut().filter(|check| !check.finished())
                {
                    check.clipboard()
                } else if let Some(check) = input_check.as_mut().filter(|check| !check.finished()) {
                    check.clipboard()
                } else {
                    &mut clipboard
                };
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
                    &mut renderer,
                    &gpu,
                    &view,
                    &scene_view,
                    event_clipboard,
                    &mut import_drops,
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
            if shell.content_dragging() {
                if offscreen && shell.desktop_pointer.is_none() {
                    dirty |= shell.content_drag_over(&editor, dt)?;
                } else {
                    track_content_drag(
                        desktop_pointer(&shell),
                        main_dpi,
                        &window,
                        &floats,
                        &mut shell,
                        &editor,
                        &scene_view,
                        dt,
                    )?;
                    shell.leave();
                    dirty = true;
                    for float in &mut floats.windows {
                        float.dirty = true;
                    }
                }
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
            match native.input(&mut shell.ui, &mut shell.input, &mut clipboard) {
                Ok(changed) => {
                    dirty |= changed;
                    // Typed text (IME commits) is input too.
                    if changed {
                        editor.input_active();
                        last_input = Some(Instant::now());
                    }
                }
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
                if offscreen
                    && matches!(
                        request,
                        crate::shell::WindowRequest::Minimize
                            | crate::shell::WindowRequest::Maximize
                            | crate::shell::WindowRequest::Fullscreen
                            | crate::shell::WindowRequest::Raise(_)
                    )
                {
                    shell.status_note = Some(
                        "Native window activation is unavailable during offscreen verification"
                            .into(),
                    );
                } else {
                    match request {
                        crate::shell::WindowRequest::Minimize => window.minimize()?,
                        crate::shell::WindowRequest::Maximize => window.toggle_maximize()?,
                        crate::shell::WindowRequest::Fullscreen => window.toggle_fullscreen()?,
                        crate::shell::WindowRequest::Raise(id) => {
                            if let Some(f) = floats.windows.iter().find(|f| f.id == id) {
                                f.window.raise();
                            }
                        }
                        crate::shell::WindowRequest::Gather => floats.gather(&window)?,
                        crate::shell::WindowRequest::Import => {
                            let folder = shell
                                .import_picker_folder
                                .take()
                                .unwrap_or_else(|| editor.state.asset_folder.clone());
                            if !offscreen {
                                file_picker::show(
                                    &window,
                                    editor.host.root(),
                                    folder,
                                    import_send.clone(),
                                );
                            }
                        }
                        crate::shell::WindowRequest::Quit => {
                            if monkey.is_none() {
                                exit_requested = shell.request_quit(&mut editor)?;
                            }
                        }
                    }
                }
                dirty = true;
            }
            for selection in imports.try_iter() {
                match selection {
                    file_picker::Selection::Files {
                        sources,
                        destination,
                    } => {
                        for source in sources {
                            import_drops.push(source, destination.clone());
                        }
                    }
                    file_picker::Selection::Cancelled => {}
                    file_picker::Selection::Failed(message) => {
                        let error: Box<dyn std::error::Error> = message.into();
                        editor.report_failure(&*error);
                    }
                }
                dirty = true;
            }
            match import_drops.frame(&editor, &mut shell, &window, main_dpi, &floats, dt) {
                Ok(changed) => {
                    dirty |= changed;
                    for float in &mut floats.windows {
                        float.dirty |= changed;
                    }
                }
                Err(error) => {
                    editor.report_failure(&*error);
                    dirty = true;
                }
            }
            match import_drops.flush_authored(&mut shell) {
                Ok(count) => dirty |= count > 0,
                Err(error) => {
                    editor.report_failure(&*error);
                    dirty = true;
                }
            }
            dirty |= shell.poll_dialogs(&mut editor)?;
            if let Some(text) = shell.take_clipboard() {
                use ara_ui_text::Clipboard;
                let destination: &mut dyn Clipboard = if let Some(check) =
                    diagnostics_check.as_mut().filter(|check| !check.finished())
                {
                    check.clipboard()
                } else if let Some(check) = history_check.as_mut().filter(|check| !check.finished())
                {
                    check.clipboard()
                } else if let Some(check) = input_check.as_mut().filter(|check| !check.finished()) {
                    check.clipboard()
                } else {
                    &mut clipboard
                };
                if let Err(error) = destination.write(&text) {
                    editor.report_failure(&*error);
                }
            }
            shell.main_rect = window.rect()?;
            let fly_dt = navigation_check
                .as_ref()
                .and_then(navigation_verify::NavigationCheck::fly_dt)
                .unwrap_or(dt);
            match controller.fly(&mut editor, &shell, fly_dt) {
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
            if let Some(route) = &mut scene_quality
                && route.step(&mut editor)?
            {
                // Navigation is input: the Scene's interaction policies see it as such.
                last_input = Some(Instant::now());
                dirty = true;
                scene_dirty = true;
            }
            let authority_scope = profile_frame.map(|_| ara_profile::scope("editor.authority"));
            let tick_at = Instant::now();
            server.pump(&mut editor);
            editor.tick();
            dirty |= shell.coalesce_rebuilds(&mut editor, |shell, editor| {
                let mut changed = false;
                for result in [
                    shell.poll_reachability(editor),
                    shell.poll_hierarchy_filters(editor),
                    shell.poll_asset_imports(editor),
                    shell.poll_asset_operations(editor),
                    shell.poll_build_options(editor),
                ] {
                    match result {
                        Ok(updated) => changed |= updated,
                        Err(error) => {
                            editor.report_failure(&*error);
                            changed = true;
                        }
                    }
                }
                if changed {
                    // Async presentation can change without an authoring/view revision.
                    // Reconcile once so progress and newly opened dialogs are reachable.
                    shell.rebuild(editor)?;
                }
                Ok(changed)
            })?;
            match shell.poll_play_job(&mut editor) {
                Ok(true) => {
                    shell.rebuild(&mut editor)?;
                    dirty = true;
                }
                Ok(false) => {}
                Err(error) => {
                    editor.report_failure(&*error);
                    dirty = true;
                }
            }
            let tick_ms = tick_at.elapsed().as_secs_f64() * 1000.;
            // The Game view, in the main window or a floating one: its size (physical
            // pixels, at its window's scale) is where embedded Play starts, what it
            // resizes to and what the camera preview renders at; new frames redraw.
            let host = shell.game_host().map(|(id, frame)| {
                let scale = match id {
                    None => Some(
                        options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor),
                    ),
                    Some(id) => floats.scale(id, &editor),
                };
                (scale, frame)
            });
            if let Some((Some(scale), frame)) = host {
                let size = frame.map(|v| (v * scale).round() as u32);
                if size[0] >= 64 && size[1] >= 64 {
                    shell.game_target = size;
                    editor.game_size = size;
                    shell.game_scale = scale;
                    editor.game_scale = scale;
                }
            }
            let was_live = shell.game_live;
            let was_fault = shell.game_fault.clone();
            let was_start = shell.game_start.clone();
            let was_play_views = shell.play_controls.revision;
            let game_frame = game.update(
                &mut editor,
                &mut shell,
                &mut gpu,
                crate::gpu::ui_device(renderer.gpu()),
            )?;
            dirty |= game_frame;
            if shell.play_controls.revision != was_play_views
                || shell.game_live != was_live
                || shell.game_fault != was_fault
                || shell.game_start != was_start
            {
                if !shell.game_live {
                    shell.game_captured = false;
                }
                // Start progress alone (the phase, bar, count, elapsed time) updates
                // the starting screen in place; anything else changes the Game view.
                let progress_only = shell.play_controls.revision == was_play_views
                    && shell.game_live == was_live
                    && shell.game_fault == was_fault;
                if !(progress_only && shell.patch_start_progress(was_start.as_ref())?) {
                    shell.rebuild(&mut editor)?;
                }
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
            match shell.poll_animation(&mut editor) {
                Some(crate::shell::LiveChange::Page) => {
                    shell.rebuild(&mut editor)?;
                    dirty = true;
                }
                Some(crate::shell::LiveChange::Canvas) => dirty = true,
                None => {}
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
            // The colour picker's eyedropper samples the last frame under the pointer,
            // or the screen outside the window.
            dirty |= eyedropper::sample(
                &mut editor,
                &mut shell,
                &mut gpu,
                &window,
                last_dpi,
                !options.offscreen(),
                &mut eyedropper_buttons,
            )?;
            let geometry_at = Instant::now();
            let mut view_ms = 0.;
            let mut stages = scene_sync::Stages::default();
            let idle = last_input.is_none_or(|t| t.elapsed() >= INTERACTION_PAUSE);
            if scene_sync.refresh(&view, &mut editor, &mut renderer, &mut scene_view, idle) {
                dirty = true;
                scene_dirty = true;
            }
            if editor.host.store().revision() != last_revision {
                last_revision = editor.host.store().revision();
                let at = Instant::now();
                // The panels' resolve of this revision (shared, not repeated).
                let next = shell.document_view(&editor)?;
                editor.report_missing_sources(&next);
                view_ms = at.elapsed().as_secs_f64() * 1000.;
                stages =
                    scene_sync.commit(&view, &next, &mut editor, &mut renderer, &mut scene_view);
                view = next;
                shell.rebuild_if_stale(&mut editor)?;
                dirty = true;
                scene_dirty = true;
            }
            if editor.extraction_revision != last_extraction {
                last_extraction = editor.extraction_revision;
                view = editor.view()?;
                scene_sync.rebuild(&view, &mut editor, &mut renderer, &mut scene_view);
                dirty = true;
                scene_dirty = true;
            }
            if editor.review_revision != last_review {
                last_review = editor.review_revision;
                let next = shell.document_view(&editor)?;
                if next.ghosts() != view.ghosts() {
                    scene_sync.rebuild(&next, &mut editor, &mut renderer, &mut scene_view);
                }
                view = next;
                dirty = true;
                scene_dirty = true;
            }
            if scene_sync.preview(&view, &mut editor, &shell, &mut renderer, &mut scene_view) {
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
            let (styles_changed, viewport_changed) = shell.reload_styles(&mut editor)?;
            dirty |= styles_changed;
            scene_dirty |= viewport_changed;
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
                    &mut scene_view,
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
                    &mut scene_view,
                    &floats,
                )?;
                dirty |= changed;
                scene_dirty |= changed;
            }
            if let Some(route) = &mut first_hour {
                // Content work, not temporal frames: a native Scene keeps submitting
                // frames while it is shown, so it never stops wanting them.
                let busy = scene_view.preview_needs_frame();
                let changed = route.step(&mut editor, &mut shell, &mut gpu, busy)?;
                dirty |= changed;
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
                    &mut scene_view,
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
                    &mut scene_view,
                )?;
                dirty |= changed;
                scene_dirty |= changed;
                if !was_finished && suite.finished() {
                    idle_samples.clear();
                    wall_samples.clear();
                }
            }
            if let Some(route) = &mut assets_verification {
                dirty |= route.step(
                    &mut editor,
                    &mut shell,
                    &mut gpu,
                    &mut ui_renderer,
                    &mut renderer,
                    &mut scene_view,
                )?;
            }
            drop(authority_scope);
            let physical = window.size()?;
            let main_drawable = !physical.contains(&0);
            if !main_drawable && floats.windows.is_empty() {
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
            let physical = if main_drawable {
                physical
            } else {
                last_physical
            };
            let dpi = options.dpi.unwrap_or_else(|| native.dpi()) * windows::ui_scale(&editor);
            if main_drawable && (physical != last_physical || dpi != last_dpi) {
                if let Some(present) = &mut present {
                    // SAFETY: Wait before replacing a live native swapchain.
                    unsafe {
                        renderer.gpu().wait_idle()?;
                        gpu.retire_present(crate::gpu::ui_device(renderer.gpu()), present);
                        *present =
                            gpu.create_present(crate::gpu::ui_device(renderer.gpu()), &window)?;
                    }
                }
                last_physical = physical;
                last_dpi = dpi;
                gpu.set_dpi(dpi)?;
                // `resize` marks the layout dirty: a rebuild with unchanged markup
                // alone would keep the old size's layout until some node changed.
                shell.ui.resize(physical.map(|v| v as f32 / dpi))?;
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
            if shell.native_maximized != window.maximized() {
                shell.native_maximized = window.maximized();
                shell.rebuild(&mut editor)?;
            }
            let mut work = shell.frame()?;
            if shell.file_drag_pending() {
                shell.refresh_file_drag_feedback(&editor);
                work += shell.frame()?;
            }
            // Resolve Scene geometry only after every native surface has settled.
            // A detached Scene shares the existing scene/render authority.
            for float in &mut floats.windows {
                if shell.enter(&float.id) {
                    let frame = shell.frame()?;
                    shell.refresh_file_drag_feedback(&editor);
                    shell.orient_axis(editor.state.camera);
                    work += frame;
                    work += shell.frame()?;
                    shell.leave();
                    float.dirty |=
                        frame.paint_nodes > 0 || frame.layout_passes > 0 || frame.restyled > 0;
                }
            }
            if let Some(button) = shell.update_chrome(&mut window, dpi) {
                caption.update(button);
            }
            if !offscreen {
                cursors::update(
                    &mut window,
                    &mut floats,
                    &mut shell,
                    controller.gizmo_state(),
                )?;
            }
            let scene_host = windows::resolve_scene_host(&editor, &shell, &floats, dpi);
            if let Some(crate::shell::ViewportRequest::DropToSurface { doc, selection }) =
                shell.viewport_request.take()
            {
                let dropped = (|| -> ara_ui::Result<bool> {
                    if editor.state.doc != doc || editor.state.selection != selection {
                        return Err(
                            "The Scene or selection changed before surface placement; try again"
                                .into(),
                        );
                    }
                    if editor.gesture.is_some() {
                        return Err(
                            "Finish the active transform gesture before dropping the selection"
                                .into(),
                        );
                    }
                    let host = scene_host
                        .as_ref()
                        .ok_or("Show the Scene view before dropping geometry to a surface")?;
                    if let Some(surface) = host.surface
                        && !shell.enter(surface)
                    {
                        return Err("The Scene window closed before surface placement".into());
                    }
                    controller.drop_to_surface(&mut editor, &mut shell, &scene_view)
                })();
                shell.leave();
                match dropped {
                    Ok(changed) => {
                        dirty |= changed;
                        scene_dirty |= changed;
                    }
                    Err(error) => {
                        editor.report_failure(&*error);
                        dirty = true;
                    }
                }
            }
            let ui_ms = ui_at.elapsed().as_secs_f64() * 1000.;
            let (mut render_ms, mut present_ms, mut presented) = (0., 0., false);
            let (mut scene_ms, mut icons_ms) = (0., 0.);
            let graph_paint = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
            drop(ui_scope);
            dirty |= work.paint_nodes > 0 || work.layout_passes > 0 || work.restyled > 0;
            // Progressive Scene frames (temporal convergence after the camera moves,
            // arriving content, GI) wait for INTERACTION_PAUSE without input, so a click
            // or keystroke never queues behind one: a ray-traced frame blocks this
            // thread for its GPU time (27-50 ms on large scenes), a native frame with
            // content work (publications, styling, GI delivery) costs it 15-70 ms during
            // imports, and even a native frame that only converges keeps the GPU busy
            // when the UI presents. Right after navigation the user is looking at the
            // Scene, so a native frame that only converges renders at once: holding it
            // back left the image unconverged between moves. Changes made by input
            // always render at once.
            if editor.state.camera != edit_before.2 {
                last_navigation = Some(Instant::now());
            }
            let converging = renderer.scene_frame_async()
                && !scene_view.preview_needs_frame()
                && last_navigation.is_some_and(|t| t.elapsed() < INTERACTION_PAUSE);
            if scene_host.is_some()
                && scene_view.needs_frame(editor.state.quality)
                && (scene_dirty
                    || converging
                    || last_input.is_none_or(|t| t.elapsed() >= INTERACTION_PAUSE))
            {
                dirty = true;
                scene_dirty = true;
            }
            dirty |= game_preview.wants_frame(&editor, &mut shell, &renderer, &scene_view);
            // Typing and committing present the UI first: the Scene follows on the next
            // frame without such input, so a keystroke or Enter never waits for a Scene
            // render. Gestures and camera moves render at once.
            let defer_scene = scene_dirty
                && (interaction_start.is_some() || probe.as_ref().is_some_and(|p| p.acted()))
                && editor.gesture.is_none()
                && editor.state.camera == edit_before.2
                && (editor.host.store().revision() != edit_before.0
                    || shell.drafts() != &edit_before.1);
            dirty |= scene_dirty;
            let mut scene_overlays = None;
            let mut gpu_surface_submissions = 0;
            let mut native_presentations = 0;
            // A window that cannot show anything (minimized, occluded; the probe's
            // unpresentable scenario) renders and presents nothing: a driver's present
            // path may stall until it shows. Offscreen verification windows render.
            let main_showable = main_drawable
                && !probe
                    .as_ref()
                    .is_some_and(responsiveness::Probe::unpresentable)
                && (offscreen || window.presentable());
            let showable = main_showable
                || floats
                    .windows
                    .iter()
                    .any(|float| offscreen || float.window.presentable());
            let mut output_requested = dirty && main_showable;
            // The previous UI frame or window blit still running after the bound (a
            // stalled present path) skips this one instead of blocking the event pump.
            // SAFETY: `present` was created on this renderer's device.
            let ui_ready = showable
                && gpu.wait_previous_for(UI_STALL_WAIT)?
                && (!main_showable
                    || match &mut present {
                        // SAFETY: This native presentation belongs to the renderer device.
                        Some(present) => unsafe { present.settled(renderer.gpu(), UI_STALL_WAIT)? },
                        None => true,
                    });
            if !ui_ready {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if dirty && ui_ready {
                let _render_scope = profile_frame.map(|_| ara_profile::scope("editor.render"));
                profile_gpu = scene_host.is_some() && scene_dirty;
                let render_start = Instant::now();
                let scene_at = Instant::now();
                if let Some(host) = &scene_host {
                    let rect = host.viewport;
                    renderer.resize(extent_scaled(rect, host.dpi))?;
                    // A Scene that cannot render yet (models still publishing, a missing
                    // source) keeps its last frame and is reported; the next change or
                    // publication retries. It never closes the editor.
                    if scene_dirty
                        && !defer_scene
                        && let Err(error) = scene_view.render(&mut renderer, &editor, &view, 1)
                    {
                        editor.report_failure(&*error);
                    }
                    scene_view.attach(&mut gpu, &renderer, editor.state.quality)?;
                    if renderer.presentation_ready() {
                        if let Some(surface) = host.surface {
                            shell.enter(surface);
                        }
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
                            &mut shell.outlines,
                            editor.host.store().revision(),
                            &shell.ui.sheet,
                        )?;
                        shell.outlines.upload(&mut ui_renderer, &mut gpu)?;
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
                        scene_overlays = Some((host.target, rect, overlays));
                        shell.leave();
                    }
                }
                // Outside Play the Game view shows the gameplay camera; its image is
                // attached (and the panel rebuilt when it appears) before the UI draws.
                if game_preview.update(
                    &mut renderer,
                    &mut scene_view,
                    &mut editor,
                    &view,
                    &mut shell,
                    &mut gpu,
                )? {
                    shell.rebuild(&mut editor)?;
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
                if scene_overlays
                    .as_ref()
                    .is_some_and(|(target, _, _)| *target == 0)
                {
                    if let Some((_, rect, overlays)) = scene_overlays.take() {
                        gpu.overlays(rect, overlay::ENTITY_IMAGE, overlays);
                    }
                } else {
                    gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
                }
                let render_at = Instant::now();
                {
                    ara_gpu::cpu_scope!("UI.render");
                    ui_renderer.render(
                        &mut gpu,
                        &shell.ui.display,
                        &shell.text.assets,
                        shell.ui.size.map(|v| v.round() as u32),
                        TextMode::Raster,
                    )?;
                    gpu_surface_submissions += 1;
                }
                render_ms = render_at.elapsed().as_secs_f64() * 1000.;
                let graph_gpu = graph_start.map(|start| start.elapsed().as_secs_f64() * 1000.);
                let present_at = Instant::now();
                if main_showable && let Some(present) = &mut present {
                    // SAFETY: UI submission is complete or GPU-ordered before the blit
                    // (one queue), image is CopySource and all native owners are live.
                    unsafe {
                        gpu.present(crate::gpu::ui_device(renderer.gpu()), present)?;
                        native_presentations += 1;
                    }
                }
                present_ms = present_at.elapsed().as_secs_f64() * 1000.;
                presented = main_showable;
                if let Some(splash) = splash.take() {
                    splash.hand_over(&window);
                }
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
                scene_dirty = defer_scene;
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
                if float.window.size()?.contains(&0) {
                    continue;
                }
                let work = shell.frame_surface(&float.id)?;
                if !(float.dirty || work.paint_nodes > 0 || work.layout_passes > 0) {
                    continue;
                }
                // As the main window: nothing to show, or a stalled UI frame or blit,
                // waits.
                let showable = offscreen || float.window.presentable();
                output_requested |= showable;
                let mut settled = || match &mut float.present {
                    // SAFETY: Each native presentation was created on this device.
                    Some(present) => unsafe { present.settled(renderer.gpu(), UI_STALL_WAIT) },
                    None => Ok(true),
                };
                if !showable || !gpu.wait_previous_for(UI_STALL_WAIT)? || !settled()? {
                    continue;
                }
                let float_dpi = options.dpi.unwrap_or_else(|| float.window.display_scale())
                    * windows::ui_scale(&editor);
                let Some(surface) = shell.surface(&float.id) else {
                    continue;
                };
                gpu.select_target(float.key);
                gpu.set_dpi(float_dpi)?;
                if scene_overlays
                    .as_ref()
                    .is_some_and(|(target, _, _)| *target == float.key)
                {
                    if let Some((_, rect, overlays)) = scene_overlays.take() {
                        gpu.overlays(rect, overlay::ENTITY_IMAGE, overlays);
                    }
                } else {
                    gpu.overlays(Default::default(), overlay::ENTITY_IMAGE, Vec::new());
                }
                ui_renderer.render(
                    &mut gpu,
                    &surface.ui.display,
                    &shell.text.assets,
                    surface.ui.size.map(|v| v.round() as u32),
                    TextMode::Raster,
                )?;
                gpu_surface_submissions += 1;
                if let Some(present) = &mut float.present {
                    // SAFETY: UI output and both native owners remain live.
                    unsafe {
                        gpu.present(crate::gpu::ui_device(renderer.gpu()), present)?;
                    }
                    native_presentations += 1;
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
            if probe.is_some() || monkey.is_some() {
                let cpu_ms = frame_start.elapsed().as_secs_f64() * 1000.;
                let ui_wait_ms = gpu.take_gpu_wait().as_secs_f64() * 1000.;
                // Unavailable when the device was replaced this frame.
                let host_wait_ms = renderer
                    .gpu()
                    .host_wait_stats()
                    .elapsed_since(Some(waits_before))
                    .map(|waited| waited.as_secs_f64() * 1000.);
                let stats = if probe.is_none() {
                    shell.take_stats()
                } else {
                    Default::default()
                };
                let sample = responsiveness::Sample {
                    offscreen_output: offscreen,
                    output_requested,
                    gpu_surface_submissions,
                    native_presentations,
                    cpu_ms,
                    host_wait_ms,
                    work_ms: host_wait_ms.map(|waited| (cpu_ms - waited - ui_wait_ms).max(0.)),
                    inject_ms,
                    input_ms,
                    ui_ms,
                    tick_ms,
                    geometry_ms,
                    view_ms,
                    extract_ms: stages.extract_ms,
                    upload_ms: stages.upload_ms,
                    patched: stages.patched,
                    native: stages.native,
                    scene_ms,
                    icons_ms,
                    render_ms,
                    gpu_ms: ui_wait_ms,
                    present_ms,
                    presented,
                    game_frame,
                    rebuilds: stats.rebuilds,
                    rebuild_ms: stats.rebuild_ms,
                    mount_ms: stats.mount_ms,
                    reconcile_ms: stats.reconcile_ms,
                    frame_ms: stats.frame_ms,
                    rows_ms: stats.rows_ms,
                    work: stats.work,
                    presentation_ready: renderer.presentation_ready(),
                    ..Default::default()
                };
                if let Some(runner) = &mut monkey {
                    runner.record_frame_with_stages(frame_start.elapsed(), &sample);
                }
                if let Some(probe) = &mut probe {
                    probe.end(&editor, &mut shell, sample);
                }
            }
            if let Some(check) = &mut drag_check {
                check.end(&editor, &mut shell, &mut gpu, &floats)?;
            } else if ux_verification.as_ref().is_some_and(|v| v.finished())
                && camera_samples.len() >= 240
            {
                drag_check = Some(drags::DragCheck::new(&options.output, &shell));
            }
            if let Some(route) = &mut first_hour {
                route.frame(
                    frame_start.elapsed().as_secs_f64() * 1000.,
                    tick_ms,
                    view_ms,
                );
            }
            if let Some(route) = &mut scene_quality {
                route.end(
                    &editor,
                    &shell,
                    &mut gpu,
                    &renderer,
                    &scene_view,
                    scene_ms,
                    presented,
                )?;
                if route.finished() {
                    route.write(&serde_json::to_value(renderer.gpu().info())?)?;
                    game.close(&mut editor, &mut gpu)?;
                    break;
                }
            }
            if let Some(runner) = &mut monkey {
                runner.observe_shell_resources(&mut editor, &shell)?;
            }
            if monkey.as_ref().is_some_and(monkey::Monkey::finished) {
                game.close(&mut editor, &mut gpu)?;
                break;
            }
            if let Some(check) = &mut content_drags {
                check.end(
                    &editor,
                    &mut shell,
                    &mut gpu,
                    &floats,
                    verification_scene_ready(&renderer, &scene_view, &editor, last_revision),
                )?;
            }
            if let Some(check) = &mut import_check {
                check.end(&editor, &shell, &mut gpu)?;
            }
            if let Some(check) = &mut asset_ops_check {
                check.end(&editor, &mut shell, &mut gpu)?;
            }
            if let Some(check) = &mut navigation_check {
                let scene_ready =
                    verification_scene_ready(&renderer, &scene_view, &editor, last_revision);
                check.end(
                    &mut editor,
                    &shell,
                    &mut gpu,
                    &scene_view,
                    frame_start.elapsed().as_secs_f64() * 1000.,
                    scene_ready,
                )?;
            }
            if let Some(check) = &mut selection_check {
                check.end(&editor, &mut shell, &mut gpu, &controller)?;
            }
            if let Some(check) = &mut hierarchy_check {
                check.end_case(&mut editor, &mut shell, &mut gpu)?;
            }
            if let Some(check) = &mut snapping_check {
                check.end(&editor, &mut shell, &mut gpu, &floats)?;
            }
            if let Some(check) = &mut collection_check {
                check.end(&editor, &mut shell, &mut gpu)?;
            }
            if let Some(check) = &mut input_check {
                check.end(
                    &editor,
                    &shell,
                    &mut gpu,
                    frame_start.elapsed().as_secs_f64() * 1000.,
                )?;
            }
            if let Some(check) = &mut prefab_check {
                check.end(&editor, &shell, &mut gpu)?;
            }
            if let Some(check) = &mut boundary_check {
                check.end(&editor, &mut shell, &mut gpu)?;
            }
            if let Some(check) = &mut history_check {
                check.end(&editor, &mut shell, &mut gpu)?;
            }
            if let Some(check) = &mut diagnostics_check {
                check.end(
                    &editor,
                    &shell,
                    &mut gpu,
                    frame_start.elapsed().as_secs_f64() * 1000.,
                )?;
            }
            if let Some(check) = &mut reachability_check {
                check.end(&editor, &shell, &mut gpu)?;
            }
            if let Some(check) = &mut play_check {
                check.end(&editor, &shell, &mut gpu)?;
                if check.finished() {
                    game.close(&mut editor, &mut gpu)?;
                    break;
                }
            }
            frames += 1;
            if probe.as_ref().is_some_and(responsiveness::Probe::finished) {
                let device = serde_json::to_value(renderer.gpu().info())?;
                if let Some(probe) = &probe {
                    probe.write(&device)?;
                }
                game.close(&mut editor, &mut gpu)?;
                break;
            }
            if options.frames.is_some_and(|limit| frames >= limit)
                || assets_verification
                    .as_ref()
                    .is_some_and(assets_verify::AssetsVerification::finished)
                || verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 5000)
                || panel_verification
                    .as_ref()
                    .is_some_and(|v| v.finished() && idle_samples.len() >= 1500)
                || ux_verification.as_ref().is_some_and(|v| {
                    v.finished()
                        && camera_samples.len() >= 240
                        && drag_check.as_ref().is_some_and(drags::DragCheck::finished)
                        && (idle_samples.len() >= 1500
                            || ux_idle_since.get_or_insert_with(Instant::now).elapsed()
                                >= UX_IDLE_PATIENCE)
                })
                || (options.verify_docking
                    && drag_check.as_ref().is_some_and(drags::DragCheck::finished))
                || journey.as_ref().is_some_and(journey::Journey::finished)
                || first_hour.as_ref().is_some_and(qa::FirstHour::finished)
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
        if let Some(check) = &drag_check {
            check.write()?;
            if !check.finished() {
                return Err("native docking verification did not finish".into());
            }
            let failed = check.failures();
            if !failed.is_empty() {
                return Err(format!("drags changed nothing: {}", failed.join(", ")).into());
            }
        }
        if journey.as_ref().is_some_and(|v| !v.finished()) {
            return Err("the journey route did not finish".into());
        }
        if let Some(check) = &content_drags {
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native behaviour journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
        }
        if options.verify_behaviours && options.behaviour_asset_ops {
            let check = import_check
                .as_ref()
                .ok_or("The OS import journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native OS import journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = asset_ops_check
                .as_ref()
                .ok_or("The Assets operation journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native Assets operation journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
        }
        if options.verify_behaviours {
            let check = snapping_check
                .as_ref()
                .ok_or("The snapping journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native snapping journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = navigation_check
                .as_ref()
                .ok_or("The navigation journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native navigation journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = selection_check
                .as_ref()
                .ok_or("The selection journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native selection journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = hierarchy_check
                .as_ref()
                .ok_or("The hierarchy workflow journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native hierarchy workflow journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = collection_check
                .as_ref()
                .ok_or("The collection journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native collection journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = input_check
                .as_ref()
                .ok_or("The native input journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native input journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = prefab_check
                .as_ref()
                .ok_or("The prefab journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native prefab journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = boundary_check
                .as_ref()
                .ok_or("The panel recovery journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native panel recovery journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = history_check
                .as_ref()
                .ok_or("The History journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native History journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
        }
        if options.verify_behaviours {
            let check = diagnostics_check
                .as_ref()
                .ok_or("The diagnostics journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native diagnostics journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = reachability_check
                .as_ref()
                .ok_or("The reachability journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native reachability journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
            let check = play_check
                .as_ref()
                .ok_or("The Play journey did not start")?;
            check.write()?;
            if !check.finished() || !check.failures().is_empty() {
                return Err(format!(
                    "Native Play journey failed: {}",
                    check.failures().join(", ")
                )
                .into());
            }
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
            let timings = json!({"targetHz":144,"requestedPixels":options.size,"logicalSize":shell.ui.size,"dpi":last_dpi,"graph500PanZoomInputToPresentMs":panel_verification.as_ref().map(|v|statistics(&v.samples)),"warmCameraToPresentMs":statistics(&camera_samples),"idleFrameIntervalMs":statistics(&wall_samples),"idleCpuMs":statistics(&idle_samples),"dirtyRenderAndPresentMs":statistics(&render_samples),"nativeInputToPresentMs":statistics(&interaction_samples),"idleSamplesMs":idle_samples,"renderSamplesMs":render_samples,"warmCameraSamplesMs":camera_samples,"idleIntervalSamplesMs":wall_samples,"interactionSamplesMs":interaction_samples,"device":renderer.gpu().info(),"note":"CPU active time excludes idle sleep; native presentation and current renderer serialize. Scripted timings are recorded separately."});
            std::fs::write(
                options.output.join("timings.json"),
                ara_json::to_canonical_string(&timings),
            )?;
        }
        Ok(())
    })();
    let monkey_result = if let Some(runner) = &mut monkey {
        runner.finish_with(&mut editor)?;
        let accepted = if runner.is_soak() {
            runner.soak_succeeded()
        } else {
            runner.succeeded()
        };
        if accepted {
            Ok(())
        } else {
            Err("Editor monkey did not pass; see monkey.json and monkey-events.jsonl".into())
        }
    } else {
        Ok(())
    };
    game_preview.destroy(&mut renderer, &mut gpu)?;
    scene_view.destroy(&mut renderer)?;
    if let Some(present) = &mut present {
        // SAFETY: Complete native presentation before destroying its owners.
        unsafe {
            renderer.gpu().wait_idle()?;
            gpu.retire_present(crate::gpu::ui_device(renderer.gpu()), present);
        }
    } else {
        gpu.wait_previous()?;
    }
    floats.destroy(&renderer, &gpu);
    result.and(monkey_result)
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
    let create_renderer =
        if crate::world::gltf::render_settings(editor.host.store().workspace())?.ray_tracing {
            Renderer::new
        } else {
            Renderer::new_raster
        };
    let mut renderer = create_renderer(
        view.scene(editor.host.root(), &editor.state, None, &mut editor.models)?,
        options.size,
        &[],
        editor.host.root().join(".ara/gpu"),
    )?;
    detect_quality(&mut editor, &renderer);
    let mut scene_view = viewport::SceneView::default();
    scene_view.render_capture(&mut renderer, &editor, &view, 64)?;
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
fn verification_scene_ready(
    renderer: &Renderer,
    scene: &viewport::SceneView,
    editor: &Editor,
    revision: u64,
) -> bool {
    revision == editor.host.store().revision()
        && renderer.presentation_ready()
        && scene.snap_ready(&editor.state.doc)
        && !scene.needs_frame(editor.state.quality)
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
    editor.action("editor.camera",json!({"target":camera.target.to_array(),"yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance,"fovYDegrees":camera.fov_y_degrees,"projection":camera.projection,"orthographicHeight":camera.orthographic_height}))?;
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
    renderer: &mut Renderer,
    gpu: &NativeUi,
    view: &crate::world::DocumentView,
    scene_view: &viewport::SceneView,
    clipboard: &mut dyn ara_ui_text::Clipboard,
    import_drops: &mut import_drops::ImportDrops,
) -> ara_ui::Result<(bool, bool, bool)> {
    if matches!(
        event,
        Event::MouseButton { pressed: true, .. } | Event::Wheel(_)
    ) {
        shell.note_pointer_chord();
    }
    if shell.game_captured && (shell.modal_active() || shell.choice_open()) {
        game_input(&Event::FocusLost, editor, shell, 1., None);
    }
    if matches!(event, Event::FileDropBegin) && shell.game_captured {
        game_input(&Event::FocusLost, editor, shell, 1., None);
    }
    if import_drops.event(&event, sdl, editor, shell, window, dpi, floats)? {
        for float in &mut floats.windows {
            float.dirty = true;
        }
        return Ok((true, false, false));
    }
    let captured_input_scale = if shell.game_captured {
        if let Some(float) = floats.windows.iter().find(|float| float.sdl == sdl) {
            crate::native::points_to_logical(
                float.window.raw(),
                float.window.display_scale().max(0.25) * windows::ui_scale(editor),
            )
        } else {
            crate::native::points_to_logical(window.raw(), dpi)
        }
    } else {
        1.
    };
    if !shell.tab_pending()
        && !shell.content_pending()
        && !shell.modal_active()
        && !shell.choice_open()
    {
        let handled = game_input(
            &event,
            editor,
            shell,
            captured_input_scale,
            floats
                .windows
                .iter()
                .find(|float| float.sdl == sdl)
                .map(|float| float.id.as_str()),
        );
        if let Some(handled) = handled {
            shell.leave();
            return Ok((handled, false, false));
        }
    }
    if shell.content_pending() {
        match &event {
            Event::Key {
                scancode: 41,
                pressed: true,
                ..
            } => {
                controller.cancel_pointer_gestures(editor, shell)?;
                return Ok((true, true, false));
            }
            Event::MouseMotion { .. } => {
                track_content_drag(
                    desktop_pointer(shell),
                    dpi,
                    window,
                    floats,
                    shell,
                    editor,
                    scene_view,
                    0.,
                )?;
                shell.leave();
                for float in &mut floats.windows {
                    float.dirty = true;
                }
                return Ok((true, false, false));
            }
            Event::MouseButton {
                button: 1,
                pressed: false,
            } if shell.content_dragging() => {
                track_content_drag(
                    desktop_pointer(shell),
                    dpi,
                    window,
                    floats,
                    shell,
                    editor,
                    scene_view,
                    0.,
                )?;
                let dropped = shell.drop_at(editor);
                shell.leave();
                shell.release_tab_capture(clipboard)?;
                controller.release(1);
                dropped?;
                for float in &mut floats.windows {
                    float.dirty = true;
                }
                return Ok((true, true, false));
            }
            _ => {}
        }
    }
    // Tab drags follow the desktop pointer: preview in whichever window is under it.
    if shell.dragged_tab().is_some() {
        match &event {
            Event::Key {
                scancode: 41,
                pressed: true,
                ..
            } => {
                controller.cancel_pointer_gestures(editor, shell)?;
                for float in &mut floats.windows {
                    float.dirty = true;
                }
                return Ok((true, true, false));
            }
            Event::MouseMotion { .. } => {
                let global = desktop_pointer(shell);
                let ui_scale = windows::ui_scale(editor);
                track_tab_drag(global, dpi, ui_scale, window, floats, shell)?;
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
                let global = desktop_pointer(shell);
                // A release can follow a layout change without another motion event.
                track_tab_drag(
                    global,
                    dpi,
                    windows::ui_scale(editor),
                    window,
                    floats,
                    shell,
                )?;
                let rect = [(global[0] - 60.) as i32, (global[1] - 14.) as i32, 420, 560];
                shell.leave();
                shell.release_tab_capture(clipboard)?;
                let dropped = shell.finish_tab_drag(editor, Some(rect));
                controller.release(1);
                dropped?;
                return Ok((true, false, false));
            }
            _ => {}
        }
    }
    if sdl == 0 || sdl == main_id {
        shell.leave();
        if matches!(event, Event::Quit) {
            return Ok((true, false, shell.request_quit(editor)?));
        }
        let press = matches!(
            event,
            Event::MouseButton {
                button: 1,
                pressed: true
            }
        );
        let result = controller.event(
            crate::native::logical_event_for(window.raw(), event, dpi),
            editor,
            shell,
            renderer,
            view,
            scene_view,
            clipboard,
        );
        if press && shell.content_pending() {
            let scale = crate::native::points_to_logical(window.raw(), dpi);
            shell.content_drag_pointer(desktop_pointer(shell).map(|v| v * scale));
        }
        return result;
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
            if let Some(present) = &mut float.present {
                // SAFETY: Wait before replacing a live native swapchain.
                unsafe {
                    renderer.gpu().wait_idle()?;
                    gpu.retire_present(crate::gpu::ui_device(renderer.gpu()), present);
                    *present =
                        gpu.create_present(crate::gpu::ui_device(renderer.gpu()), &float.window)?;
                }
            }
            shell.resize_surface(&id, size.map(|v| v as f32 / float_dpi))?;
            shell.rebuild(editor)?;
            Ok((true, false, false))
        }
        event => {
            let press = matches!(
                event,
                Event::MouseButton {
                    button: 1,
                    pressed: true
                }
            );
            let event = crate::native::logical_event_for(float.window.raw(), event, float_dpi);
            if !shell.enter(&id) {
                return Ok((false, false, false));
            }
            let result =
                controller.event(event, editor, shell, renderer, view, scene_view, clipboard);
            if press && shell.content_pending() {
                let scale = crate::native::points_to_logical(window.raw(), dpi);
                shell.content_drag_pointer(desktop_pointer(shell).map(|v| v * scale));
            }
            shell.leave();
            result
        }
    }
}
/// The desktop pointer tab drags follow: the OS one, or a verification route's.
fn desktop_pointer(shell: &Shell) -> [f32; 2] {
    shell.desktop_pointer.unwrap_or_else(Window::global_mouse)
}
/// Resolve one content drag in its destination surface; all desktop distances use
/// the main window's logical scale while each surface keeps its own local scale.
#[allow(clippy::too_many_arguments)]
fn track_content_drag(
    global: [f32; 2],
    dpi: f32,
    window: &Window,
    floats: &windows::Floats,
    shell: &mut Shell,
    editor: &Editor,
    scene_view: &viewport::SceneView,
    seconds: f32,
) -> ara_ui::Result<()> {
    shell.leave();
    let main_scale = crate::native::points_to_logical(window.raw(), dpi);
    let desktop = global.map(|value| value * main_scale);
    let on_surface = if let Some(float) = floats.at(global) {
        let scale = crate::native::points_to_logical(
            float.window.raw(),
            float.window.display_scale() * windows::ui_scale(editor),
        );
        if shell.enter(&float.id) {
            shell.pointer = [
                (global[0] - float.rect[0] as f32) * scale,
                (global[1] - float.rect[1] as f32) * scale,
            ];
            true
        } else {
            false
        }
    } else {
        let rect = window.rect()?;
        shell.pointer = [
            (global[0] - rect[0] as f32) * main_scale,
            (global[1] - rect[1] as f32) * main_scale,
        ];
        global[0] >= rect[0] as f32
            && global[1] >= rect[1] as f32
            && global[0] < (rect[0] + rect[2]) as f32
            && global[1] < (rect[1] + rect[3]) as f32
    };
    shell.content_drag_pointer(desktop);
    if on_surface {
        shell.content_drag_over(editor, seconds)?;
        if shell.content_asset_dragging()
            && let Some(rect) = shell.viewport()
            && let Some(hit) = scene_view.surface_hit(&editor.state.camera, rect, shell.pointer)
        {
            shell.content_surface_hit_details(hit.position, hit.normal, hit.vertex);
            if let Some(draw) = hit.draw {
                shell.content_material_hit(editor, draw)?;
            }
        }
    } else {
        shell.content_drag_outside();
    }
    // Keep the destination entered until release so drop_at resolves that surface.
    Ok(())
}
/// Point the tab-drag preview at the window under a desktop point. A floating
/// window maps it with the same DPI × UI scale its own input uses.
fn track_tab_drag(
    global: [f32; 2],
    dpi: f32,
    ui_scale: f32,
    window: &Window,
    floats: &windows::Floats,
    shell: &mut Shell,
) -> ara_ui::Result<()> {
    if let Some(float) = floats.at(global) {
        let scale = crate::native::points_to_logical(
            float.window.raw(),
            float.window.display_scale() * ui_scale,
        );
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
        let scale = crate::native::points_to_logical(window.raw(), dpi);
        shell.pointer = [
            (global[0] - m[0] as f32) * scale,
            (global[1] - m[1] as f32) * scale,
        ];
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
/// While the Game view holds the keyboard and mouse, raw input goes to the shown Play
/// client (the player maps it exactly like its own window). Esc, or pressing outside
/// the view, gives them back; F5-F8 stay with the editor, whose modifier state is mirrored.
/// `None` lets the editor handle the event; `Some(redraw)` consumed it.
fn game_input(
    event: &Event,
    editor: &mut Editor,
    shell: &mut Shell,
    points_to_logical: f32,
    surface: Option<&str>,
) -> Option<bool> {
    if !shell.game_captured {
        return None;
    }
    if let Some(surface) = surface {
        if !shell.enter(surface) {
            let result = game_input_current(&Event::FocusLost, editor, shell, points_to_logical);
            shell.leave();
            return result;
        }
    } else {
        shell.leave();
    }
    let result = game_input_current(event, editor, shell, points_to_logical);
    shell.leave();
    result
}

/// Capture hit testing and raw player forwarding on the already selected surface.
fn game_input_current(
    event: &Event,
    editor: &mut Editor,
    shell: &mut Shell,
    points_to_logical: f32,
) -> Option<bool> {
    use ara_play_host::embed::DeviceEvent;
    if matches!(
        event,
        Event::Key { pressed: true, .. }
            | Event::MouseButton { pressed: true, .. }
            | Event::MouseMotion { .. }
    ) {
        editor.input_active();
    }
    let release = |editor: &mut Editor, shell: &mut Shell| {
        shell.game_captured = false;
        if let (Some(world), Some(surfaces)) = (&editor.play_world, &editor.play_surfaces) {
            surfaces.device(world, shell.game_client, vec![DeviceEvent::FocusLost]);
        }
        let _ = shell.rebuild(editor);
    };
    let events = match *event {
        Event::FocusLost => {
            release(editor, shell);
            return None;
        }
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
            let inside = shell
                .game_input_rect()
                .is_some_and(|rect| rect.contains(shell.pointer[0], shell.pointer[1]));
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
            // Captured input uses the same window-density mapping as editor input.
            shell.pointer = position.map(|value| value * points_to_logical);
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
    if matches!(
        event,
        Event::Key {
            scancode: 224..=231,
            ..
        }
    ) {
        // Both owners need modifier transitions for chords such as Shift+F5.
        None
    } else {
        Some(false)
    }
}

/// The GPU's automatic preset: what Auto (and an Auto project default) resolve to in
/// the Scene view's menus, status bar and `quality` gating.
fn detect_quality(editor: &mut Editor, renderer: &Renderer) {
    use ara_render_fidelity::presets::{Capabilities, QualityPreset};
    editor.state.detected_quality = Some(QualityPreset::detect(&Capabilities::of(renderer.gpu())));
    editor.state.apply_settings();
}
