//! The studio: Ara's editor on stillkit (D34). One app window, CPU-drawn and redrawn only
//! where something changed, with the Scene view as a native GPU layer under a transparent
//! hole: the renderer's image and the scene overlays (grid, outlines, gizmo) are composited
//! on the GPU and presented into it (no copy through the CPU UI), and the UI's
//! toolbars, gizmo labels and menus composite over it.
//!
//! The studio is a client of the Editor's own surfaces: navigation and selection go through
//! `editor.*` actions and content through `cmd.apply`, exactly what agents call over MCP, so
//! every person's action has an agent equivalent. The host loop below owns the renderer and
//! the transport; the runtime owns the [`Studio`] state and its view.
mod about;
mod agent_work;
mod agents;
mod animator;
mod asset_create;
mod asset_inspector;
mod asset_ops;
mod build_page;
mod capture;
mod chrome;
mod commands;
mod components;
mod compositor;
mod compound;
mod create;
mod curve;
mod dock;
mod game;
mod game_preview;
mod gizmo;
mod graph;
pub mod headless;
mod hierarchy;
mod inspector;
mod layer;
mod lighting;
mod loading;
mod material_stats;
mod mesh_paint;
mod panels;
mod player;
mod portals;
mod preferences;
mod present;
pub(crate) mod probes;
mod profiler;
mod quit;
mod rect_tool;
mod references;
mod running;
mod scene;
mod scene_overlays;
pub(crate) mod scene_sync;
mod scripts;
pub(crate) mod setup;
mod shortcuts;
mod snapping;
pub mod state;
mod style;
#[cfg(test)]
mod tests;
mod trace;
mod unavailable;
mod vfx;
pub(crate) mod viewport;
mod zoom;

use std::time::{Duration, Instant};

use stillkit::el::{Runtime, SystemClipboard};
use stillkit::platform::current::host::{modifiers, reduced_motion, wait, wake_at};
use stillkit::{Caption, Chrome, Event, Kind, Role, Surface, next_event};

use crate::transport::Server;
use crate::world::EditWorldView as _;
use crate::{Editor, settings};
use scene_sync::SceneSync;
pub use setup::Options;
pub use state::Studio;
use viewport::SceneView;

/// V, held during a move to snap vertex to vertex (a virtual-key code).
const VERTEX_KEY: u16 = 0x56;

/// The Scene view's native layer.
pub(crate) const SCENE_LAYER: &str = "scene";

/// How long closing waits for running probe work to retire (it is cancelled first, so this
/// is normally the time its in-flight GPU work needs). Past it the GPU objects are left to
/// the process exit rather than destroyed under that work.
const CLOSE_DRAIN: Duration = Duration::from_secs(5);

/// Open the project in the studio and run until its window closes.
pub fn run(options: Options) -> anyhow::Result<()> {
    let user_settings = settings::user_settings_path()?;
    let mut editor = Editor::open_config(
        &options.project,
        Some(user_settings),
        ara_host::HostConfig::default(),
    )?;
    setup::prepare(&mut editor, &options).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut server = Server::start(editor.host.root())?;
    eprintln!(
        "Ara editor authority listening on 127.0.0.1:{}",
        server.port
    );
    let _ = stillkit::set_identity(stillkit::Identity {
        prefix: "ara-editor",
        desktop_classes: &[],
    });
    let title = format!("Ara | {}", loading::project_name(&editor));
    // Native layers need a GPU UI surface: DirectComposition over child HWNDs on Windows,
    // a Vulkan surface over Wayland subsurfaces on Linux.
    #[cfg(any(windows, target_os = "linux"))]
    stillkit::platform::set_render(stillkit::platform::Render::Gpu);
    let mut surface = Surface::new(Kind::App { min: (1024, 640) }, &title, Role::Window)?;
    surface.set_caption(Caption::Compact);
    let platform = surface.scale();
    let (sw, sh) = stillkit::platform::current::host::screen_size();
    // A bounded run opens at the requested size in device pixels (`--size`); otherwise the
    // studio's own.
    let (want_w, want_h) = if options.bounded() {
        (options.size[0] as f32, options.size[1] as f32)
    } else {
        (1600.0 * platform, 1000.0 * platform)
    };
    let w = (want_w as i32).min(sw - 40);
    let h = (want_h as i32).min(sh - 80);
    surface.place(((sw - w) / 2, (sh - h) / 2, w, h));
    let size = surface.client_size();
    // A small window shows the UI a little larger (`zoom`): the runtime's scale is the
    // platform's times that zoom, and the layout sees the window in design px at that scale.
    let scale = platform * zoom::for_width(size.0 as f32 / platform);
    let mut studio = Studio::new(editor, (size.0 as f32 / scale, size.1 as f32 / scale));
    studio.frame = frame(&surface, scale);
    studio.local.scene.message = "Preparing the scene renderer…".into();
    let theme = studio.look.theme.clone();
    let background = studio.look.c.background.0;
    let mut rt = Runtime::new(studio, chrome::view, theme, size, scale)?;
    rt.set_background(Some(background));
    rt.set_clipboard(SystemClipboard);
    rt.set_reduced_motion(reduced_motion());
    surface.set_background(background);
    rt.frame(Duration::ZERO);
    rt.present(&mut surface, (0, 0))?;
    chrome_parts(&surface, &rt);
    // Floating windows are native windows (one runtime; each shows its region).
    rt.touch(|s| s.local.desktop.on = true);
    let mut portals = portals::Portals::default();
    // In the background (automated captures, tests beside a person's work) the window
    // shows without activating or taking focus; otherwise it comes forward, active.
    if options.hidden() {
        // `--frames N`: never shown. The loop and the protocol server run as usual.
        rt.touch(|s| s.local.background = true);
    } else if options.background || options.screenshot {
        rt.touch(|s| s.local.background = true);
        surface.show();
    } else {
        surface.show_active(false);
    }

    // The renderer starts on a worker; the window keeps drawing and taking input meanwhile.
    let scene_rect = rt
        .natives()
        .into_iter()
        .find(|(n, _)| &**n == SCENE_LAYER)
        .map(|(_, r)| r);
    let extent = scene_rect.map_or([1280, 720], |r| {
        [r.width().max(64.0) as u32, r.height().max(64.0) as u32]
    });
    let started = Instant::now();
    let (scene, ray_tracing, quality) = rt.touch(|s| -> anyhow::Result<_> {
        let view = s.editor.view()?;
        let settings = crate::world::gltf::render_settings(s.editor.host.store().workspace())?;
        let scene = view.scene(
            s.editor.host.root(),
            &s.editor.state,
            None,
            &mut s.editor.models,
        )?;
        Ok((scene, settings.ray_tracing, settings.quality.into()))
    })?;
    let cache = rt.state().editor.host.root().join(".ara/gpu");
    let mut quit = false;
    let renderer = viewport::prepare_renderer(
        scene,
        extent,
        present_extensions(),
        cache,
        ray_tracing,
        quality,
        |_| {
            rt.touch(|s| {
                server.pump(&mut s.editor);
                s.editor.tick();
            });
            quit |= pump(&mut rt, &mut surface, &mut portals, started, None)?;
            Ok(!quit)
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(mut renderer) = renderer else {
        return Ok(());
    };
    let device = serde_json::to_string(renderer.gpu().info()).unwrap_or_default();
    rt.update(|s| {
        s.local.scene = state::SceneStatus {
            ready: true,
            message: String::new(),
            fps: None,
        };
        s.local.device = device;
    });
    let mut view = rt.touch(|s| s.editor.view())?;
    let mut scene_view = SceneView::default();
    let mut sync =
        rt.touch(|s| SceneSync::new(s.editor.models.revision(), s.editor.host.store().revision()));
    let mut revision = rt.state().editor.host.store().revision();
    let mut extraction = rt.state().editor.extraction_revision;
    // SAFETY: the renderer outlives the compositor (retired and dropped below, before
    // it); this thread submits for both.
    let mut compositor =
        unsafe { compositor::Compositor::new(&renderer, rt.state().editor.state.light)? };
    // SAFETY: as the compositor's.
    let mut game = unsafe { game::Game::new(&renderer)? };
    // SAFETY: as the compositor's.
    let mut graph_layer = unsafe { layer::Layer::new(&renderer, [])? };
    let mut shown_extent = [0, 0];
    // The tag of the Scene frame last composited (`Renderer::shown_frame_tag`).
    let mut shown_tag = u64::MAX;
    // The running drag's snap source, captured at its first snapped move.
    let mut snap: Option<snapping::Snap> = None;
    // The selection the Rect tool's bounds were last read for.
    let mut rect_for: Option<Vec<String>> = None;
    // The graph canvas's size when last drawn.
    let mut graph_extent = ("", [0, 0]);
    // A composite is owed (something changed) but has not presented yet.
    let mut composite = false;
    // The Scene's size while a resize is under way, and when it last changed.
    let mut sizing: Option<([u32; 2], Instant)> = None;
    let mut overlay_key = 0_u64;
    // Derived probe jobs (`render.probe.*`): served against the Scene view rendered here.
    let mut probe_owner = probes::Owner::default();
    // Native layers placed on the last pass: one shown again presents at once (on Wayland
    // a hidden layer loses its picture).
    let mut placed: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    // The last native-layer failure reported, so a recurring one is said once.
    let mut layer_failure: Option<String> = None;
    let mut scene_dirty = true;
    // The import gate: held until the project's imports settle (see `loading`).
    let mut gate = Some(rt.touch(|s| loading::Gate::new(&s.editor, options.unattended())));
    // A bounded run counts frames and ends itself: `--frames N`, or `--screenshot` (60 unless
    // given). Presented Scene frames count when the window shows; hidden, the loop's own.
    let frame_limit = options.bounded().then(|| options.frames.unwrap_or(60));
    let mut frames = 0_u64;
    let mut last_frame = Instant::now();
    let mut fps = 0.0_f32;
    let mut camera = rt.state().editor.state.camera;
    let mut logs_at = Instant::now() - Duration::from_secs(1);
    let mut probe_report_at = Instant::now() - Duration::from_secs(1);
    let mut last_fly = Instant::now();
    let (mut profile_at, mut was_recording) = (Instant::now(), false);
    // `--panel NAME`: open it the studio's way (tool pages dock in the centre).
    if let Some(panel) = &options.panel {
        let id = format!("window.{panel}");
        rt.update(|s| commands::run(s, &id, &crate::catalog::Target::None));
    }
    rt.update(|s| {
        let store = s.editor.host.store();
        s.assets = Some(crate::assets::Browser::new(
            s.editor.host.root(),
            store.schema(),
            store.workspace().reference_root(),
        ));
    });
    loop {
        // Vulkan's Scene may run on its own queue: finish external reads before recycling images.
        #[cfg(any(windows, target_os = "linux"))]
        if !compositor.scene_ready(&renderer)? {
            // Cancelling a bake must not wait for the GPU reads this gate waits on.
            rt.touch(|s| {
                server.pump(&mut s.editor);
                agent_work::deliver_cancellations(&mut s.editor, &mut probe_owner, &mut scene_view);
            });
            if pump(
                &mut rt,
                &mut surface,
                &mut portals,
                started,
                Some(&mut game),
            )? {
                break;
            }
            // The authority keeps serving (protocol, imports, agents, Play) while the
            // Scene's images are still being read; only GPU work waits.
            let synced = rt.touch(|s| {
                server.pump(&mut s.editor);
                s.editor.tick();
                s.editor.sync()
            });
            if synced {
                rt.update(|_| {});
                scene_dirty = true;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        // While profiling records, every pass is a frame (the Profiler panel reads them).
        let profiling = rt.touch(|s| {
            let profiler = s.editor.host.profiler();
            profiler.is_recording() && profiler.begin_frame().is_ok()
        });
        let mut pass = trace::Pass::begin();
        pass.phase("studio.events");
        let before_events = rt.state().editor.state.camera;
        if pump(
            &mut rt,
            &mut surface,
            &mut portals,
            started,
            Some(&mut game),
        )? {
            break;
        }
        hierarchy::unhover(&mut rt);
        // Whether the person's own input moved the view this pass (Pilot Camera).
        let mut steered = rt.state().editor.state.camera != before_events;
        pass.mark("events+ui");
        pass.phase("studio.authority");
        // The authority: protocol requests, imports, agent work.
        let (changed, sync_changed) = rt.touch(|s| {
            // A Scene switch reaches the probe jobs before requests about the new one.
            for _ in 0..2 {
                if !probe_owner.invalidate_scene(&mut s.editor) {
                    break;
                }
                s.editor.tick();
            }
            server.pump(&mut s.editor);
            s.editor.tick();
            // Cancellation reaches a running bake before the next Scene GPU work.
            scene_dirty |=
                probe_owner.poll_cancellations(&mut s.editor, &mut scene_view, &mut renderer);
            (
                s.editor.host.store().revision() != revision,
                s.editor.sync(),
            )
        });
        if sync_changed {
            rt.update(|_| {});
            scene_dirty = true;
        }
        // The import card shows while imports settle, and the native views stay hidden so a
        // half-imported scene is never presented as finished. A person can let themselves in.
        if let Some(g) = gate.as_mut() {
            if rt.state().local.import_released {
                g.release("a person");
            }
            if rt.state().editor.import_gate.take_release() {
                g.release("an agent");
            }
            let standing = g.step();
            for line in g.take_log() {
                eprintln!("ara-editor: {line}");
            }
            if rt.state().local.import_gate != standing {
                let next = standing.clone();
                rt.update(|s| s.local.import_gate = next);
            }
            if standing.is_none() {
                gate = None;
                scene_dirty = true;
                composite = true;
            }
        }
        let gated = gate.is_some();
        rt.state().editor.import_gate.set_holding(gated);
        // Commands queued by `editor.command` (agents over MCP) and `--play`: run at this
        // pass boundary through the same handlers as the menus.
        // A preset or rebind changed in Settings: menus and the palette show it.
        if rt.touch(commands::refresh_keys) {
            rt.update(|_| {});
        }
        let queued = rt.touch(|s| std::mem::take(&mut s.editor.queued_commands));
        if !queued.is_empty() {
            rt.update(|s| {
                for (id, target) in &queued {
                    commands::run(s, id, target);
                }
            });
        }
        pass.mark("authority");
        pass.phase("studio.assets");
        // Assets and the diagnostics bus.
        if rt.touch(|s| s.assets.as_mut().is_some_and(crate::assets::Browser::poll)) {
            rt.update(|_| {});
        }
        // Tile pictures for the open folder, rendered off-thread.
        rt.touch(|s| {
            let folder = s.editor.state.asset_folder.clone();
            let root = s.editor.host.root().to_owned();
            if let Some(browser) = &s.assets {
                let wanted: Vec<String> = browser
                    .items
                    .iter()
                    .filter(|a| {
                        state::Previews::previewable(&a.kind)
                            && a.path.rsplit_once('/').map_or("", |(p, _)| p) == folder
                    })
                    .map(|a| a.path.clone())
                    .collect();
                s.previews.want(&root, wanted.iter().map(String::as_str));
            }
        });
        if rt.touch(|s| s.previews.poll()) {
            rt.update(|_| {});
        }
        if logs_at.elapsed() >= Duration::from_millis(400) {
            logs_at = Instant::now();
            if rt.touch(Studio::poll_logs) {
                rt.update(|_| {});
            }
        }
        pass.mark("assets+logs");
        pass.phase("studio.scene_sync");
        let idle = last_frame.elapsed() >= Duration::from_millis(250);
        if rt.touch(|s| sync.refresh(&view, &mut s.editor, &mut renderer, &mut scene_view, idle)) {
            scene_dirty = true;
        }
        if changed {
            let next = rt.touch(|s| {
                revision = s.editor.host.store().revision();
                s.editor.view()
            })?;
            let stages = rt.touch(|s| {
                sync.commit(&view, &next, &mut s.editor, &mut renderer, &mut scene_view)
            });
            trace::log(|| {
                format!(
                    "scene commit: patched {} native {}, {:.1} ms",
                    stages.patched, stages.native, stages.upload_ms
                )
            });
            view = next;
            rt.update(Studio::resolve);
            scene_dirty = true;
        }
        if rt.state().editor.extraction_revision != extraction {
            extraction = rt.state().editor.extraction_revision;
            view = rt.touch(|s| s.editor.view())?;
            rt.touch(|s| sync.rebuild(&view, &mut s.editor, &mut renderer, &mut scene_view));
            scene_dirty = true;
        }
        // The gizmo drag or the scrubbed field, shown before it commits.
        let preview = rt.touch(|s| inspector::Preview::of(s));
        if rt.touch(|s| {
            sync.preview(
                &view,
                &mut s.editor,
                &preview,
                &mut renderer,
                &mut scene_view,
            )
        }) {
            scene_dirty = true;
        }
        // The Rect tool's bounds: the selection's presented geometry (selected parents take
        // their children's), read again when the scene or the selection changes.
        let wanted = rt.touch(|s| {
            let st = &s.editor.state;
            (st.tool == crate::state::Tool::Rect
                && !st.hide_gizmos
                && !st.selection.is_empty()
                && s.local.rect_drag.is_none())
            .then(|| st.selection.clone())
        });
        match wanted {
            Some(selection) if scene_dirty || rect_for.as_ref() != Some(&selection) => {
                let channels = compositor.channels(&renderer, &view, revision);
                let entities = view.entities();
                let rotation = rect_tool::frame_rotation(entities, &selection);
                let bounds = scene_view
                    .bounds_in(
                        channels,
                        |address| crate::world::bounds::selected(entities, address, &selection),
                        rotation,
                    )
                    .map(|[min, max]| rect_tool::RectBounds { rotation, min, max });
                rect_for = Some(selection);
                if rt.state().local.rect_bounds != bounds {
                    rt.update(|s| s.local.rect_bounds = bounds);
                }
            }
            Some(_) => {}
            None => rect_for = None,
        }
        pass.mark("scene sync");
        pass.phase("studio.scene");
        // The Scene view: render where its element shows and present into its layer.
        let rect = native_rect(&rt, SCENE_LAYER, gated);
        surface.place_native_layer(SCENE_LAYER, rect);
        composite |= shown_again(&mut placed, SCENE_LAYER, rect.is_some());
        // A hidden (minimized) window shows no frames: render none. A covered one skips
        // presents on its own (the 1 ms present wait).
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let shown = surface.is_visible();
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let shown = true;
        let on_screen = rect.filter(|r| shown && r.width() >= 8.0 && r.height() >= 8.0);
        let scene_presented = on_screen.is_some();
        if let Some(rect) = on_screen {
            let actual = [rect.width() as u32, rect.height() as u32];
            // Reallocating the renderer's targets takes hundreds of milliseconds, so a live
            // resize keeps the last frame (the layer stretches it) and the renderer follows
            // once the size has held for a moment.
            if actual != shown_extent && sizing.is_none_or(|(e, _)| e != actual) {
                sizing = Some((actual, Instant::now()));
            }
            let settled = shown_extent == [0, 0]
                || sizing.is_none_or(|(_, at)| at.elapsed() >= Duration::from_millis(120));
            let extent = if settled { actual } else { shown_extent };
            if settled {
                sizing = None;
            }
            let now = rt.state().editor.state.camera;
            let camera_moved = now != camera;
            camera = now;
            // What the overlays draw besides the camera: the selection, tool and theme.
            let key = rt.touch(|s| overlays_key(s));
            let restyled = key != overlay_key;
            overlay_key = key;
            let render = camera_moved
                || scene_dirty
                || scene_view.needs_frame(rt.state().editor.state.quality)
                || shown_extent != extent;
            if render {
                renderer
                    .resize(extent)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                agent_work::render(
                    &mut rt,
                    &mut probe_owner,
                    &mut scene_view,
                    &mut renderer,
                    &view,
                );
            }
            pass.mark(if render {
                "scene render"
            } else {
                "scene check"
            });
            pass.phase("studio.composite");
            // An asynchronous Scene frame lands passes after it was submitted: show it then.
            let landed = renderer.shown_frame_tag() != shown_tag;
            composite |= render || restyled || landed;
            let layer = (composite && renderer.presentation_ready())
                .then(|| layer_target(&surface, SCENE_LAYER, &mut rt, &mut layer_failure))
                .flatten();
            if let Some(layer) = layer {
                let scale = rt.scale();
                let shown = rt.touch(|s| {
                    // Overlays draw with the camera and drag pose of the frame on screen,
                    // so a lagging frame carries its gizmos and grid with it.
                    let mut gizmo = gizmo_state(s);
                    let live = s.editor.state.camera;
                    if let Some(shown) = scene_view.shown_frame(&renderer) {
                        s.editor.state.camera = shown.camera;
                        gizmo.offset = if shown.revision == s.editor.host.store().revision() {
                            shown.offset
                        } else {
                            glam::Mat4::IDENTITY
                        };
                    }
                    let changed = s.editor.agent_tints();
                    // SAFETY: the layer is the Scene view's own, alive while the window is.
                    let done = unsafe {
                        compositor.frame(
                            &mut renderer,
                            &mut scene_view,
                            &view,
                            &s.editor.state,
                            &changed,
                            gizmo,
                            revision,
                            layer,
                            extent,
                            scale,
                        )
                    };
                    drop(changed);
                    s.editor.state.camera = live;
                    done
                })?;
                if !shown {
                    // The last composite is still on the GPU: try again next pass.
                    pass.mark("composite skipped");
                } else {
                    composite = false;
                    if landed {
                        let tag = renderer.shown_frame_tag();
                        trace::log(|| format!("scene frame {tag} shown"));
                    }
                    shown_tag = renderer.shown_frame_tag();
                    shown_extent = extent;
                    if render {
                        let dt = last_frame.elapsed().as_secs_f32().max(1e-4);
                        fps = if fps == 0.0 {
                            1.0 / dt
                        } else {
                            fps * 0.9 + 0.1 / dt
                        };
                        last_frame = Instant::now();
                        let shown = (fps.min(999.0), 1000.0 / fps.max(1.0));
                        rt.touch(|s| s.local.scene.fps = Some(shown));
                    }
                }
            }
            if render {
                scene_dirty = false;
            }
            pass.mark("composite");
            // A click in the view: the entity under it through the channel map.
            if let Some(pick) = rt.touch(|s| s.local.pick.take()) {
                let size = renderer.size();
                let at = [
                    ((pick.u * size[0] as f32) as u32).min(size[0].saturating_sub(1)),
                    ((pick.v * size[1] as f32) as u32).min(size[1].saturating_sub(1)),
                ];
                let entity = match compositor.pick(&mut renderer, &view, revision, at) {
                    Ok(entity) => entity,
                    Err(error) => {
                        rt.touch(|s| s.editor.report_failure(&*error));
                        None
                    }
                };
                rt.update(|s| {
                    use crate::catalog::{Target, menus::Context};
                    match (&entity, pick.context) {
                        // A right click keeps a selection it lands in.
                        (Some(e), Some(_)) if s.editor.state.selection.contains(e) => {}
                        (Some(e), _) => s.select(vec![e.clone()], pick.additive),
                        (None, _) if !pick.additive => s.select(Vec::new(), false),
                        (None, _) => {}
                    }
                    if let Some(at) = pick.context {
                        s.local.context = Some(match entity {
                            Some(e) => (Context::Entity, Target::Entity(e), at),
                            None => (Context::Viewport, Target::None, at),
                        });
                    }
                });
            }
        } else if agent_work::needs_unpresented_render(&probe_owner, shown_extent) {
            // A probe bake advances only through Scene renders. With the Scene covered (the
            // Lighting page in front) or the window hidden, render at the size last shown and
            // present nothing; the frame shows if the Scene comes back.
            if scene_dirty || scene_view.needs_frame(rt.state().editor.state.quality) {
                renderer
                    .resize(shown_extent)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                agent_work::render(
                    &mut rt,
                    &mut probe_owner,
                    &mut scene_view,
                    &mut renderer,
                    &view,
                );
                scene_dirty = false;
            }
            pass.mark("scene unpresented");
        }
        // Probe jobs and agents' captures are served every pass, the Scene shown or not.
        pass.phase("studio.agents");
        // The Scene is "presented" once a frame of it has been composited into its layer.
        let presented = shown_extent != [0, 0] && scene_presented;
        scene_dirty |= agent_work::serve(
            &mut rt,
            &mut probe_owner,
            &mut scene_view,
            &mut renderer,
            presented,
        );
        pass.mark("agents");
        pass.phase("studio.game");
        // A move with vertex or surface snapping: the presented geometry answers it.
        if let Some(request) = rt.touch(|s| s.local.snap.take()) {
            let channels = compositor.channels(&renderer, &view, revision);
            let target_of = |id: u64| channels.target_of(id).map(str::to_owned);
            rt.update(|s| {
                if snap.as_ref().is_none_or(|c| !c.live(&s.editor, &view)) {
                    snap = snapping::Snap::capture(
                        &s.editor,
                        &view,
                        &scene_view,
                        &target_of,
                        &request,
                    );
                }
                let Some(capture) = snap.as_ref() else {
                    return;
                };
                match capture.delta(&s.editor, &scene_view, &request) {
                    Ok((delta, said)) => {
                        if let Some(gesture) = s.editor.gesture.as_mut() {
                            gesture.translate_world(delta);
                        }
                        note(s, said);
                    }
                    Err(why) => note(s, why),
                }
            });
            scene_dirty = true;
        }
        if rt.state().local.gizmo.is_none() {
            snap = None;
        }
        // Drop to Surface (End): the presented geometry answers its rays.
        if rt.touch(|s| std::mem::take(&mut s.local.drop_to_surface)) {
            let channels = compositor.channels(&renderer, &view, revision);
            rt.update(|s| {
                let planned =
                    crate::surface_drop::plan(&s.editor, &scene_view, |id| channels.target_of(id));
                let result = planned.and_then(|plan| match plan {
                    Some((commands, base)) => s
                        .editor
                        .apply(commands, "Drop selection to surface", Some(base))
                        .map(drop)
                        .map_err(Into::into),
                    None => Ok(()),
                });
                if let Err(error) = result {
                    s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                }
            });
        }
        // Mesh Paint: the Scene view's presses and drags as world rays on the selection's
        // meshes; a press on none of them selects as a click would.
        let input = rt.touch(|s| std::mem::take(&mut s.local.paint_input));
        // The ring follows the pointer only when it moved (a hit test seeds every mesh).
        let hover = rt
            .touch(|s| s.local.paint_hover.take())
            .filter(|_| rt.state().editor.state.paint.is_some());
        if (!input.is_empty() || hover.is_some()) && rt.state().local.scene.ready {
            let size = renderer.size();
            let ray = |s: &Studio, (u, v): (f32, f32)| {
                s.editor
                    .state
                    .camera
                    .ray(size, [u * 2.0 - 1.0, 1.0 - v * 2.0])
            };
            rt.update(|s| {
                for event in input {
                    use state::PaintInput;
                    let result = match event {
                        PaintInput::Down(u, v) => match ray(s, (u, v)) {
                            Some(r) => {
                                s.painter
                                    .begin(&mut s.editor, &view, &scene_view, r)
                                    .map(|hit| {
                                        if !hit {
                                            s.local.pick = Some(scene::Pick {
                                                u,
                                                v,
                                                additive: false,
                                                context: None,
                                            });
                                        }
                                    })
                            }
                            None => Ok(()),
                        },
                        PaintInput::Move(u, v) => match ray(s, (u, v)) {
                            Some(r) => s.painter.drive(&mut s.editor, r).map(drop),
                            None => Ok(()),
                        },
                        PaintInput::Up => s.painter.finish(&mut s.editor, true),
                        PaintInput::Cancel => s.painter.finish(&mut s.editor, false),
                    };
                    if let Err(error) = result {
                        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
                    }
                }
                s.local.painting = s.painter.active();
                // The brush ring: the radius at the hit's distance, in view px.
                if let Some((u, v)) = hover {
                    s.local.paint_ring = (|| {
                        let settings = s.editor.state.paint?;
                        let ray = ray(s, (u, v))?;
                        let point =
                            crate::painter::Painter::hit(&s.editor, &view, &scene_view, ray)?;
                        let camera = s.editor.state.camera;
                        let distance = (point - camera.position()).length().max(1e-3);
                        let rect = s.local.rects.get("scene/view")?;
                        let half = (camera.fov_y_degrees.to_radians() * 0.5).tan();
                        let px = settings.radius / (distance * half) * rect.height() * 0.5;
                        Some((u * rect.width(), v * rect.height(), px))
                    })();
                }
            });
            scene_dirty = true;
        }
        // Mesh Paint off: no ring.
        if rt.state().editor.state.paint.is_none() && rt.state().local.paint_ring.is_some() {
            rt.update(|s| s.local.paint_ring = None);
        }
        // The Game view: the gameplay camera's picture, or Play's frames, in its own layer.
        let game_rect = native_rect(&rt, game::GAME_LAYER, gated);
        surface.place_native_layer(game::GAME_LAYER, game_rect);
        if shown_again(&mut placed, game::GAME_LAYER, game_rect.is_some()) {
            game.redraw();
        }
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let visible = surface.is_visible();
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let visible = true;
        // Play embeds its player at the Game view's size (`Editor::embed_play`).
        if let Some(r) = game_rect.filter(|r| r.width() >= 64.0 && r.height() >= 64.0) {
            // A fixed resolution plays at its own size, at 1x; the view scales it to fit.
            let fixed = rt.touch(|s| {
                let aspect = s
                    .editor
                    .state
                    .settings
                    .get("game.aspect")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("free")
                    .to_owned();
                game::resolution(&aspect)
            });
            let (size, scale) = match fixed {
                Some(size) => (size, 1.0),
                None => ([r.width() as u32, r.height() as u32], rt.scale()),
            };
            rt.touch(|s| {
                s.editor.game_size = size;
                s.editor.game_scale = scale;
            });
        }
        let game_layer =
            match game_rect.filter(|r| visible && r.width() >= 8.0 && r.height() >= 8.0) {
                Some(r) => layer_target(&surface, game::GAME_LAYER, &mut rt, &mut layer_failure)
                    .map(|layer| (layer, [r.width() as u32, r.height() as u32])),
                None => None,
            };
        if game_layer.is_some() || rt.touch(|s| game.wants_frame(&s.editor, &renderer, &scene_view))
        {
            let scale = rt.scale();
            // Input goes to the client Play's controls act on.
            game.client = rt.state().play.client;
            game.zoom = game::zoom(rt.state());
            let status = rt.touch(|s| {
                let (resolved, revision) = (s.resolved.clone(), s.editor.host.store().revision());
                // SAFETY: the layer is the Game view's own, alive while the window is.
                unsafe {
                    game.frame(
                        &mut renderer,
                        &mut scene_view,
                        &mut s.editor,
                        &view,
                        &resolved,
                        revision,
                        game_layer,
                        scale,
                    )
                }
            });
            match status {
                Ok(status) if status != rt.state().local.game => {
                    rt.update(|s| s.local.game = status)
                }
                Ok(_) => {}
                Err(error) => rt.touch(|s| s.editor.report_failure(&*error)),
            }
        }
        // Play ended: the keyboard and mouse come back.
        if rt.state().local.game_captured && rt.state().editor.state.playing.is_none() {
            rt.update(|s| s.local.game_captured = false);
        }
        // The Material and Animation pages' canvases, each in its own layer.
        for (domain, name) in graph::PAGES {
            let rect = native_rect(&rt, name, gated);
            surface.place_native_layer(name, rect);
            if shown_again(&mut placed, name, rect.is_some()) {
                graph_extent = ("", [0, 0]);
            }
            let Some(r) = rect.filter(|r| visible && r.width() >= 8.0 && r.height() >= 8.0) else {
                continue;
            };
            let extent = [r.width() as u32, r.height() as u32];
            rt.touch(|s| graph::sync(s, domain));
            rt.touch(graph::previews);
            if (rt.state().graph.dirty || graph_extent != (name, extent))
                && graph_layer.ready(&renderer)?
                && let Some(layer) = layer_target(&surface, name, &mut rt, &mut layer_failure)
            {
                let scale = rt.scale();
                // SAFETY: the layer is the page's own, alive while the window is.
                let drawn = rt.touch(|s| unsafe {
                    graph::draw(s, &mut graph_layer, &renderer, layer, extent, scale)
                });
                match drawn {
                    Ok(true) => graph_extent = (name, extent),
                    Ok(false) => {}
                    Err(error) => rt.touch(|s| s.editor.report_failure(&*error)),
                }
            }
        }
        // The VFX page's live preview: frames while it is on screen, paused otherwise.
        // A running export: its stage, cooking progress and result.
        if rt.touch(|s| s.builder.poll_build(&mut s.editor)) {
            rt.update(|_| {});
        }
        // Play's runtime controls: their running job, and Simulate once the player is live.
        let ready = rt.state().local.game.live;
        if rt.touch(|s| s.play.poll(&mut s.editor, ready)) {
            rt.update(|_| {});
        }
        if rt.touch(|s| std::mem::take(&mut s.play.release)) {
            rt.update(|s| s.local.game_captured = false);
        }
        // The Profiler's player: its targets, its frames while it records.
        let profiler_shown =
            rt.find("profiler/record").is_some() || rt.find("profiler/stop").is_some();
        if rt.touch(|s| profiler::poll(s, profiler_shown)) {
            rt.update(|_| {});
        }
        // C#: a reload in Play, module discovery for the IDE.
        if rt.touch(scripts::poll) {
            rt.update(|_| {});
        }
        // File operations (move, rename, duplicate, delete) on their jobs.
        if !rt.state().local.asset_jobs.is_empty() && rt.touch(asset_ops::poll_jobs) {
            rt.update(|_| {});
        }
        // File imports: a finished one shows its files in Assets at once.
        if rt.touch(|s| s.importer.poll(&mut s.editor)) {
            rt.update(|s| {
                if let Some(b) = s.assets.as_ref() {
                    b.refresh();
                }
            });
        }
        // Agents: the clients are looked for once the Connect rows show; answers redraw.
        if rt.find("agents/connect").is_some() && !rt.state().local.agent_setups.probed {
            rt.update(agents::probe_clients);
        }
        if rt.touch(|s| s.local.agent_setups.poll()) {
            rt.update(|_| {});
        }
        // The Lighting page reads the renderer's probe report while it shows, so Recapture
        // knows whether this quality can capture (and says why when it cannot).
        if probe_report_at.elapsed() >= Duration::from_secs(1)
            && rt.find("lighting/probe/refresh").is_some()
        {
            probe_report_at = Instant::now();
            let version = |s: &Studio| s.local.probe_status.as_ref().map(|r| r.version);
            let before = version(rt.state());
            if rt.touch(|s| lighting::refresh_probes(s).is_ok()) && version(rt.state()) != before {
                rt.update(|_| {});
            }
        }
        let vfx_visible = rt.find("vfx/preview").is_some();
        if rt.touch(|s| vfx::update(s, vfx_visible)) {
            rt.update(|_| {});
        }
        pass.mark("game");
        pass.phase("studio.fly");
        // Fly: right button held, WASD (and Q/E down and up) move the camera at the view
        // speed setting, three times as fast with Shift.
        let flying = rt.state().local.flying && !rt.state().local.held.is_empty();
        if flying {
            let dt = last_fly.elapsed().as_secs_f32().min(0.1);
            let shift = modifiers().shift;
            let before_fly = rt.state().editor.state.camera;
            rt.touch(|s| fly(s, dt, shift));
            steered |= rt.state().editor.state.camera != before_fly;
        }
        last_fly = Instant::now();
        // A piloted Camera follows the view; a finished move commits.
        if rt.state().local.pilot.active() {
            rt.update(|s| {
                scene::poll_pilot(s, steered);
            });
        }
        pass.end();
        if profiling {
            rt.touch(|s| {
                let _ = s
                    .editor
                    .host
                    .profiler()
                    .end_frame(Default::default(), Default::default());
            });
        }
        // The Profiler panel reads a copy, refreshed four times a second while recording
        // and once when it stops.
        let recording = rt.touch(|s| s.editor.host.profiler().is_recording());
        if (recording && profile_at.elapsed() >= Duration::from_millis(250))
            || (!recording && was_recording)
        {
            profile_at = Instant::now();
            rt.update(|s| {
                s.editor.host.profiler().collect();
                s.profile = std::sync::Arc::new(s.editor.host.profiler().profile().clone());
            });
        }
        if recording != was_recording {
            rt.update(|s| s.local.profiling = recording);
        }
        was_recording = recording;
        if let Some(limit) = frame_limit {
            if options.hidden() || presented {
                frames += 1;
            }
            if frames >= limit {
                if options.screenshot {
                    capture::screenshot(
                        &mut rt,
                        Some((&mut renderer, &scene_view)),
                        &options.output,
                    )?;
                }
                break;
            }
        }
        let busy = flying
            || scene_view.needs_frame(rt.state().editor.state.quality)
            || renderer.scene_frame_pending()
            || composite
            // A job wakes the loop only while the Scene can render for it.
            || agent_work::needs_unpresented_render(&probe_owner, shown_extent)
            || rt.touch(|s| vfx::running(s))
            || rt.touch(|s| game.wants_frame(&s.editor, &renderer, &scene_view));
        // One wake for the runtime's timers or the next scene frame; none when idle.
        let next = [
            rt.wake_at().map(|at| started + at),
            busy.then(|| Instant::now() + Duration::from_millis(8)),
            // A bounded run counts frames: it never idles for its half second.
            frame_limit.map(|_| Instant::now() + Duration::from_millis(16)),
            // The card's clock and the import feed are read on this wake while it holds.
            gated.then(|| Instant::now() + Duration::from_millis(100)),
            sizing.map(|(_, at)| at + Duration::from_millis(125)),
            Some(Instant::now() + Duration::from_millis(500)),
        ]
        .into_iter()
        .flatten()
        .min();
        wake_at(next);
        // Sleep until an event, that wake, or an animation frame: never spin.
        wait();
    }
    // Probe work retires with the Scene view it read, before the renderer goes. The window
    // goes first so the wait is never an unresponsive window, and it is bounded.
    surface.hide();
    rt.touch(|s| probe_owner.begin_drain(&mut s.editor));
    let draining = Instant::now();
    let mut retired = true;
    loop {
        let done = rt.touch(|s| -> ara_ui::Result<bool> {
            server.pump(&mut s.editor);
            s.editor.tick();
            probe_owner.poll(&mut s.editor, &mut scene_view, &mut renderer)?;
            s.editor.tick();
            Ok(!probe_owner.busy() && !s.editor.probe_captures.busy())
        });
        match done {
            Ok(true) => break,
            Ok(false) if draining.elapsed() < CLOSE_DRAIN => {
                std::thread::sleep(Duration::from_millis(8))
            }
            Ok(false) => {
                eprintln!(
                    "ara-editor: probe work did not retire: {}",
                    ara_json::to_canonical_string(&probe_owner.resource_report())
                );
                retired = false;
                break;
            }
            Err(error) => {
                eprintln!("ara-editor: probe work failed while closing: {error}");
                retired = false;
                break;
            }
        }
    }
    if !retired {
        // A bake still has work on the GPU: destroying its renderer under it is worse than
        // leaving the GPU objects to the process exit, which follows at once.
        std::mem::forget((scene_view, graph_layer, compositor, game, renderer));
        return Ok(());
    }
    renderer.wait_scene_frame().ok();
    rt.touch(|s| game.retire(&mut s.editor, &mut renderer));
    graph_layer.retire(&renderer);
    compositor.retire(&renderer);
    drop(compositor);
    scene_view
        .destroy(&mut renderer)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

/// Where native layer `name` shows: its element's rect, or nowhere while the import gate
/// holds, so a half-imported scene is never presented as finished.
fn native_rect(rt: &Runtime<Studio>, name: &str, gated: bool) -> Option<stillkit::Rect> {
    if gated {
        return None;
    }
    rt.natives()
        .into_iter()
        .find(|(n, _)| &**n == name)
        .map(|(_, rect)| rect)
}

/// Records whether native layer `name` is placed this pass; true when it was not on the
/// last one (shown again: present it now, since a Wayland layer loses its picture while
/// hidden).
fn shown_again(
    placed: &mut std::collections::BTreeSet<&'static str>,
    name: &'static str,
    now: bool,
) -> bool {
    match now {
        true => placed.insert(name),
        false => {
            placed.remove(name);
            false
        }
    }
}

/// Native layer `name` to present into, or `None` when the platform can't give it now (a
/// full compositor socket, say): that pass skips it, and the failure is reported once,
/// never ending the editor.
fn layer_target(
    surface: &Surface,
    name: &str,
    rt: &mut Runtime<Studio>,
    reported: &mut Option<String>,
) -> Option<present::Target> {
    match surface.native_layer(name).and_then(present::target) {
        Ok(layer) => {
            *reported = None;
            Some(layer)
        }
        Err(error) => {
            let said = format!("{error:#}");
            if reported.as_deref() != Some(said.as_str()) {
                rt.touch(|s| s.editor.report_failure(&*anyhow::anyhow!("{name}: {said}")));
                *reported = Some(said);
            }
            None
        }
    }
}

/// Whether this session can show the studio's views (native layers) before anything opens:
/// on Linux it connects to the Wayland compositor and checks for subsurfaces; elsewhere the
/// platform always can. The error says what is missing.
pub fn can_host() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    stillkit::platform::current::surface::native_layer_support()?;
    Ok(())
}

/// Vulkan instance extensions a presentation surface needs (none on Metal).
fn present_extensions() -> Vec<std::ffi::CString> {
    let names: &[&str] = if cfg!(target_os = "macos") {
        &[]
    } else if cfg!(windows) {
        &["VK_KHR_surface", "VK_KHR_win32_surface"]
    } else {
        &["VK_KHR_surface", "VK_KHR_wayland_surface"]
    };
    names
        .iter()
        .filter_map(|n| std::ffi::CString::new(*n).ok())
        .collect()
}

/// W, A, S, D, Q and E as virtual keys: the fly keys while the right button is held.
pub(crate) const FLY_KEYS: [u16; 6] = [0x57, 0x41, 0x53, 0x44, 0x51, 0x45];

/// One fly step of `dt` seconds from the held keys.
fn fly(s: &mut Studio, dt: f32, fast: bool) {
    let held = |vk: u16| f32::from(u8::from(s.local.held.contains(&vk)));
    let direction = glam::Vec3::new(
        held(0x44) - held(0x41),
        held(0x45) - held(0x51),
        held(0x53) - held(0x57),
    );
    if direction.length_squared() == 0.0 || dt.is_nan() || dt <= 0.0 {
        return;
    }
    let speed = s
        .editor
        .state
        .settings
        .get("viewport.cameraSpeed")
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite())
        .unwrap_or(4.0)
        .clamp(0.5, 50.0) as f32;
    s.editor.state.camera.fly(
        direction.normalize(),
        dt * speed * if fast { 3.0 } else { 1.0 },
    );
}

/// The gizmo handles to light: the one under the pointer and the one dragged.
fn gizmo_state(s: &Studio) -> crate::overlay::GizmoState {
    // While dragging, the gizmo draws at the previewed pose, not the committed one.
    let offset = match (&s.local.gizmo, &s.editor.gesture) {
        (Some(_), Some(g)) => g.world_delta(s.editor.state.selection.first().map(String::as_str)),
        _ => glam::Mat4::IDENTITY,
    };
    crate::overlay::GizmoState {
        hover: s.local.gizmo_hover,
        active: s.local.gizmo.as_ref().map(|d| d.handle),
        offset,
        icon: None,
    }
}

/// A digest of the editor state the Scene overlays draw besides the camera.
fn overlays_key(s: &Studio) -> u64 {
    use std::hash::{Hash, Hasher};
    let state = &s.editor.state;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", gizmo_state(s)).hash(&mut h);
    (
        &state.selection,
        state.tool as u8,
        state.local,
        state.centre,
        state.light,
        state.hide_gizmos,
        state.game_view,
    )
        .hash(&mut h);
    h.finish()
}

/// How far the pointer may stray during a press that still counts as a click (design px).
const CLICK_SLOP: f32 = 4.0;
/// How long a press may be held and still count as a click.
const CLICK_TIME: Duration = Duration::from_millis(500);

/// Whether press `e` ends as a click rather than a drag, decided as Unity does: by the
/// farthest the pointer went while the button was down (a drag that comes back to its
/// start is still a drag) and how long it was held, never by where it let go. Feed it every
/// phase of a press; it answers on the release.
pub(super) fn click(s: &mut Studio, e: &stillkit::el::PointerEvent) -> bool {
    use stillkit::el::PointerPhase;
    let travel = e.travel().x.abs() + e.travel().y.abs();
    match e.phase {
        PointerPhase::Down => {
            s.local.press = Some((Instant::now(), 0.0));
            false
        }
        PointerPhase::Move => {
            if let Some((_, far)) = s.local.press.as_mut() {
                *far = far.max(travel);
            }
            false
        }
        PointerPhase::Up => s
            .local
            .press
            .take()
            .is_some_and(|(at, far)| far.max(travel) < CLICK_SLOP && at.elapsed() < CLICK_TIME),
        PointerPhase::Cancel => {
            s.local.press = None;
            false
        }
        PointerPhase::Hover => false,
    }
}

/// Shows `message` in the status bar for a few seconds (what a request did).
pub(super) fn note(s: &mut Studio, message: impl Into<String>) {
    s.local.note = Some((message.into(), Instant::now()));
}

/// The window's own controls: the system's on macOS, drawn ones elsewhere.
fn frame(surface: &Surface, scale: f32) -> state::Frame {
    state::Frame {
        inset: surface.system_controls().map_or(0.0, |r| r.right / scale),
        controls: (!cfg!(target_os = "macos")).then(|| surface.is_maximized()),
    }
}

/// The title bar's free space drags the window; drawn controls are the platform's.
fn chrome_parts(surface: &Surface, rt: &Runtime<Studio>) {
    let mut parts = Vec::new();
    if let Some(bar) = rt.find(ara_kit::ui::TITLE_BAR) {
        parts.push((bar, Chrome::Caption));
    }
    for (key, part) in [
        (ara_kit::ui::MINIMIZE, Chrome::Minimize),
        (ara_kit::ui::MAXIMIZE, Chrome::Maximize),
        (ara_kit::ui::CLOSE, Chrome::Close),
    ] {
        if let Some(rect) = rt.find(key) {
            parts.push((rect, part));
        }
    }
    surface.set_chrome(&parts);
}

/// Whether the studio should close: File › Exit does as the window's close button does, and so
/// does a human session's `daemon.shutdown` (the installed-package smoke and Hub stop the editor
/// this way; the studio ignored it for a while, so the editor kept running).
fn closing(studio: &Studio) -> bool {
    studio.local.quit || studio.editor.host.stopping()
}

/// Handles window events, runs a UI frame and presents it; whether the window closed.
/// While the game has the keyboard and mouse (`game`, once the renderer runs), its events
/// go to the player instead: Esc, leaving the window or a click outside the Game view give
/// them back; F5 to F8 stay the editor's Play controls. Floating windows' own windows
/// (`portals`) send their input into the same runtime and show their regions of its frame.
fn pump(
    rt: &mut Runtime<Studio>,
    surface: &mut Surface,
    portals: &mut portals::Portals,
    started: Instant,
    mut game: Option<&mut game::Game>,
) -> anyhow::Result<bool> {
    if closing(rt.state()) {
        return Ok(true);
    }
    match rt.touch(|s| s.local.window.take()) {
        Some(state::WindowRequest::Minimize) => surface.minimize(),
        Some(state::WindowRequest::Maximize) => surface.toggle_maximize(),
        Some(state::WindowRequest::Fullscreen) => surface.toggle_fullscreen(),
        None => {}
    }
    portals.sync(rt, surface)?;
    while let Some((id, mut event)) = next_event() {
        let floating = portals.owns(id);
        if id != surface.id() && !floating {
            continue;
        }
        portals.map(rt, surface, id, &mut event);
        if floating && portals.window_event(rt, id, &event) {
            continue;
        }
        if !floating
            && rt.state().local.game_captured
            && let Some(game) = game.as_deref_mut()
        {
            let inside = |p: &stillkit::Pointer| {
                rt.natives()
                    .into_iter()
                    .find(|(n, _)| &**n == game::GAME_LAYER)
                    .is_some_and(|(_, r)| {
                        p.at.x >= r.left
                            && p.at.x <= r.right
                            && p.at.y >= r.top
                            && p.at.y <= r.bottom
                    })
            };
            let release = match &event {
                Event::Key(0x1B) | Event::Deactivated | Event::Leave => true,
                Event::Press(p, _) => !inside(p),
                _ => false,
            };
            if release {
                game.release(&rt.state().editor);
                rt.update(|s| s.local.game_captured = false);
                if matches!(event, Event::Key(0x1B)) {
                    continue;
                }
            } else if !matches!(event, Event::Key(0x74..=0x77) | Event::KeyUp(0x74..=0x77))
                && game.input(&rt.state().editor, &event)
            {
                continue;
            }
        }
        if !matches!(event, Event::Move(..)) {
            trace::log(|| format!("event {event:?}"));
        }
        match event {
            Event::Close => rt.update(quit::request),
            Event::Resized(w, h) => {
                // The zoom follows the window's width (`zoom`).
                let platform = surface.scale();
                rt.set_scale(platform * zoom::for_width(w as f32 / platform));
                rt.resize((w, h));
                let (size, frame) = (
                    (w as f32 / rt.scale(), h as f32 / rt.scale()),
                    frame(surface, rt.scale()),
                );
                rt.update(|s| {
                    s.size = size;
                    s.frame = frame;
                });
            }
            Event::DpiChanged(dpi) => {
                let platform = dpi as f32 / 96.0;
                rt.set_scale(platform * zoom::for_width(rt.size().0 as f32 / platform));
            }
            // A person activated a window opened in the background: it behaves as usual.
            Event::Activated => rt.touch(|s| s.local.background = false),
            // Files from another app import into the folder Assets shows.
            Event::FilesDropped(_) => {
                let files = stillkit::take_dropped_files();
                if !files.is_empty() {
                    rt.update(|s| asset_ops::import(s, files, None));
                }
            }
            Event::Key(vk) => {
                if FLY_KEYS.contains(&vk) && rt.state().local.flying {
                    rt.touch(|s| s.local.held.insert(vk));
                }
                // V held during a move snaps vertex to vertex.
                if vk == VERTEX_KEY && rt.state().local.gizmo.is_some() {
                    rt.touch(|s| s.local.vertex_key = true);
                    continue;
                }
                rt.key_with(vk, modifiers());
            }
            Event::KeyUp(vk) => {
                rt.touch(|s| {
                    s.local.held.remove(&vk);
                    if vk == VERTEX_KEY {
                        s.local.vertex_key = false;
                    }
                });
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Event::Char(c) => rt.type_text(c.encode_utf8(&mut [0; 4])),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Event::Preedit => {
                let (text, at) = stillkit::platform::current::ime::preedit();
                rt.preedit(&text, at);
            }
            Event::SettingChanged => rt.set_reduced_motion(reduced_motion()),
            #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
            Event::VisibilityChanged => rt.set_visible(surface.is_visible()),
            Event::Invoke(hit) => match surface.take_accessibility_action(hit) {
                Some(action) => {
                    rt.accessibility_action(hit, &action);
                }
                None => rt.handle(&event),
            },
            event => {
                rt.set_modifiers(modifiers());
                rt.handle(&event);
            }
        }
    }
    if rt.touch(Studio::sync_look) {
        let (theme, background) = (
            rt.state().look.theme.clone(),
            rt.state().look.c.background.0,
        );
        rt.set_theme(theme);
        rt.set_background(Some(background));
        surface.set_background(background);
        rt.update(|_| {});
    }
    rt.sync_text_input(surface);
    let painted = trace::time("ui frame", Duration::from_millis(3), || {
        rt.frame(started.elapsed().saturating_sub(rt.now()))
    });
    if let Some(key) = rt.touch(|s| s.local.focus.take())
        && !rt.state().local.background
    {
        rt.focus(key);
    }
    if let Some(text) = rt.touch(|s| s.local.copy.take()) {
        stillkit::el::Clipboard::set(&mut SystemClipboard, &text);
    }
    if painted {
        if let Some(d) = rt.damage() {
            trace::log(|| {
                format!(
                    "damage {:.0}x{:.0} at {:.0},{:.0}",
                    d.width(),
                    d.height(),
                    d.left,
                    d.top
                )
            });
        }
        trace::time("ui present", Duration::from_millis(3), || {
            rt.present(surface, (0, 0))
        })?;
        chrome_parts(surface, rt);
        portals.present(rt)?;
        // Layout feedback the view reads next time: where splits and the Scene view are.
        let mut keys = dock::rect_keys(&rt.state().editor.state.layout);
        keys.push("scene/view".into());
        keys.push("profiler/track".into());
        keys.push("graph/view".into());
        keys.push("vfx/preview".into());
        keys.push("game/area".into());
        // The Assets body's height: tiles step down to fit it (`panels::fitting`).
        keys.push("assets/body".into());
        // The path bar's room: folders that do not fit fold into "…" (`panels::crumbs_fit`).
        keys.push("assets/crumbs".into());
        keys.push("assets/search".into());
        keys.push("vfx/stacks".into());
        keys.extend(
            scene_overlays::OVERLAYS
                .iter()
                .map(|d| format!("scene/overlay/{}", d.id)),
        );
        // While a tab is dragged: every area and tab, to drop it among.
        if rt.state().local.tab_drag.is_some() {
            keys.extend(dock::all_drag_keys(rt.state()));
        }
        // While a component is dragged: the Inspector sections it can drop among.
        if rt.state().local.component_drag.is_some() {
            keys.extend(inspector::drop_keys(rt.state()));
        }
        // While an asset is dragged: the folders it can drop on.
        if rt.state().local.asset_drag.is_some() {
            keys.extend(panels::drop_keys(rt.state()));
        }
        let s = rt.scale();
        let rects = keys
            .into_iter()
            .filter_map(|k| {
                rt.find(&k).map(|r| {
                    (
                        k,
                        stillkit::Rect::new(r.left / s, r.top / s, r.width() / s, r.height() / s),
                    )
                })
            })
            .collect();
        // The Assets panel's tile size and path bar follow their measured room: a measurement
        // that changes what fits redraws it (a `touch` alone would leave the old one up).
        let tiles = |s: &Studio| {
            (
                panels::fitting(
                    s.local.assets_view.size,
                    s.local.rects.get("assets/body").map(|r| r.height()),
                ),
                ["assets/crumbs", "assets/search"]
                    .map(|k| s.local.rects.get(k).map(|r| r.width().round() as i32)),
            )
        };
        let before = tiles(rt.state());
        rt.touch(|s| {
            s.local.rects = rects;
            scene_overlays::measure(s);
        });
        if tiles(rt.state()) != before {
            rt.update(|_| {});
        }
    }
    rt.sync_text_input(surface);
    if !portals.set_cursor(rt) {
        surface.set_cursor(rt.cursor());
    }
    portals.track(rt, surface);
    Ok(false)
}
