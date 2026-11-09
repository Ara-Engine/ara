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
mod agents;
mod chrome;
mod commands;
mod components;
mod compositor;
mod create;
mod dock;
mod game;
mod gizmo;
mod hierarchy;
mod inspector;
mod panels;
mod preferences;
mod scene;
pub mod state;
mod style;
#[cfg(test)]
mod tests;
mod trace;

use std::time::{Duration, Instant};

use anyhow::Context as _;
use stillkit::el::{Runtime, SystemClipboard};
use stillkit::platform::current::host::{modifiers, reduced_motion, wait, wake_at};
use stillkit::{Caption, Chrome, Event, Kind, Role, Surface, next_event};

use crate::app::{Options, scene_sync::SceneSync, viewport::SceneView};
use crate::transport::Server;
use crate::world::EditWorldView as _;
use crate::{Editor, settings};
pub use state::Studio;

/// The Scene view's native layer.
pub(crate) const SCENE_LAYER: &str = "scene";

/// Open the project in the studio and run until its window closes.
pub fn run(options: Options) -> anyhow::Result<()> {
    let user_settings = settings::user_settings_path()?;
    let mut editor = Editor::open_config(
        &options.project,
        Some(user_settings),
        ara_host::HostConfig::default(),
    )?;
    crate::app::setup::prepare(&mut editor, &options).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut server = Server::start(editor.host.root())?;
    eprintln!(
        "Ara editor authority listening on 127.0.0.1:{}",
        server.port
    );
    let _ = stillkit::set_identity(stillkit::Identity {
        prefix: "ara-editor",
        desktop_classes: &[],
    });
    let title = format!(
        "Ara | {}",
        editor
            .host
            .root()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let mut surface = Surface::new(Kind::App { min: (1024, 640) }, &title, Role::Window)?;
    surface.set_caption(Caption::Compact);
    let scale = surface.scale();
    let (sw, sh) = stillkit::platform::current::host::screen_size();
    let w = ((1600.0 * scale) as i32).min(sw - 40);
    let h = ((1000.0 * scale) as i32).min(sh - 80);
    surface.place(((sw - w) / 2, (sh - h) / 2, w, h));
    let size = surface.client_size();
    let mut studio = Studio::new(editor, (size.0 as f32 / scale, size.1 as f32 / scale));
    studio.frame = frame(&surface);
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
    surface.show_active(false);

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
    let renderer = crate::app::viewport::prepare_renderer(
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
            quit |= pump(&mut rt, &mut surface, started, None)?;
            Ok(!quit)
        },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(mut renderer) = renderer else {
        return Ok(());
    };
    rt.update(|s| {
        s.local.scene = state::SceneStatus {
            ready: true,
            message: String::new(),
            fps: None,
        };
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
    let mut shown_extent = [0, 0];
    // The tag of the Scene frame last composited (`Renderer::shown_frame_tag`).
    let mut shown_tag = u64::MAX;
    // A composite is owed (something changed) but has not presented yet.
    let mut composite = false;
    // The Scene's size while a resize is under way, and when it last changed.
    let mut sizing: Option<([u32; 2], Instant)> = None;
    let mut overlay_key = 0_u64;
    let mut scene_dirty = true;
    let mut last_frame = Instant::now();
    let mut fps = 0.0_f32;
    let mut camera = rt.state().editor.state.camera;
    let mut logs_at = Instant::now() - Duration::from_secs(1);
    let mut last_fly = Instant::now();
    rt.update(|s| {
        let store = s.editor.host.store();
        s.assets = Some(crate::shell::assets::Browser::new(
            s.editor.host.root(),
            store.schema(),
            store.workspace().reference_root(),
        ));
    });
    loop {
        let mut pass = trace::Pass::begin();
        if pump(&mut rt, &mut surface, started, Some(&mut game))? {
            break;
        }
        pass.mark("events+ui");
        // The authority: protocol requests, imports, agent work.
        let (changed, sync_changed) = rt.touch(|s| {
            server.pump(&mut s.editor);
            s.editor.tick();
            (
                s.editor.host.store().revision() != revision,
                s.editor.sync(),
            )
        });
        if sync_changed {
            rt.update(|_| {});
            scene_dirty = true;
        }
        // Commands queued by `editor.command` (agents over MCP) and `--play`: run at this
        // pass boundary through the same handlers as the menus.
        let queued = rt.touch(|s| std::mem::take(&mut s.editor.queued_commands));
        if !queued.is_empty() {
            rt.update(|s| {
                for (id, target) in &queued {
                    commands::run(s, id, target);
                }
            });
        }
        pass.mark("authority");
        // Assets and the diagnostics bus.
        if rt.touch(|s| {
            s.assets
                .as_mut()
                .is_some_and(crate::shell::assets::Browser::poll)
        }) {
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
        pass.mark("scene sync");
        // The Scene view: render where its element shows and present into its layer.
        let rect = rt
            .natives()
            .into_iter()
            .find(|(n, _)| &**n == SCENE_LAYER)
            .map(|(_, r)| r);
        surface.place_native_layer(SCENE_LAYER, rect);
        // A hidden (minimized) window shows no frames: render none. A covered one skips
        // presents on its own (the 1 ms present wait).
        #[cfg(any(windows, target_os = "macos"))]
        let shown = surface.is_visible();
        #[cfg(not(any(windows, target_os = "macos")))]
        let shown = true;
        if let Some(rect) = rect.filter(|r| shown && r.width() >= 8.0 && r.height() >= 8.0) {
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
                if let Err(error) = rt.touch(|s| {
                    // Frames carry the drag pose they show (`SceneView::shown_frame`).
                    scene_view.gizmo_offset = gizmo_state(s).offset;
                    scene_view.render(&mut renderer, &s.editor, &view, 1)
                }) {
                    rt.touch(|s| s.editor.report_failure(&*error));
                }
            }
            pass.mark(if render {
                "scene render"
            } else {
                "scene check"
            });
            // An asynchronous Scene frame lands passes after it was submitted: show it then.
            let landed = renderer.shown_frame_tag() != shown_tag;
            composite |= render || restyled || landed;
            if composite && renderer.presentation_ready() {
                let layer = metal_layer(surface.native_layer(SCENE_LAYER)?)?;
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
            // A click in the view: the entity under it through the channel map.
            pass.mark("composite");
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
                    use crate::shell::commands::{Target, menus::Context};
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
        }
        // The Game view: the gameplay camera's picture, or Play's frames, in its own layer.
        let game_rect = rt
            .natives()
            .into_iter()
            .find(|(n, _)| &**n == game::GAME_LAYER)
            .map(|(_, r)| r);
        surface.place_native_layer(game::GAME_LAYER, game_rect);
        #[cfg(any(windows, target_os = "macos"))]
        let visible = surface.is_visible();
        #[cfg(not(any(windows, target_os = "macos")))]
        let visible = true;
        // Play embeds its player at the Game view's size (`Editor::embed_play`).
        if let Some(r) = game_rect.filter(|r| r.width() >= 64.0 && r.height() >= 64.0) {
            let (size, scale) = ([r.width() as u32, r.height() as u32], rt.scale());
            rt.touch(|s| {
                s.editor.game_size = size;
                s.editor.game_scale = scale;
            });
        }
        let game_layer =
            match game_rect.filter(|r| visible && r.width() >= 8.0 && r.height() >= 8.0) {
                Some(r) => Some((
                    metal_layer(surface.native_layer(game::GAME_LAYER)?)?,
                    [r.width() as u32, r.height() as u32],
                )),
                None => None,
            };
        if game_layer.is_some() || rt.touch(|s| game.wants_frame(&s.editor, &renderer, &scene_view))
        {
            let scale = rt.scale();
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
        pass.mark("game");
        // Fly: right button held, WASD (and Q/E down and up) move the camera at the view
        // speed setting, three times as fast with Shift.
        let flying = rt.state().local.flying && !rt.state().local.held.is_empty();
        if flying {
            let dt = last_fly.elapsed().as_secs_f32().min(0.1);
            let shift = modifiers().shift;
            rt.touch(|s| fly(s, dt, shift));
        }
        last_fly = Instant::now();
        pass.end();
        let busy = flying
            || scene_view.needs_frame(rt.state().editor.state.quality)
            || renderer.scene_frame_pending()
            || composite
            || rt.touch(|s| game.wants_frame(&s.editor, &renderer, &scene_view));
        // One wake for the runtime's timers or the next scene frame; none when idle.
        let next = [
            rt.wake_at().map(|at| started + at),
            busy.then(|| Instant::now() + Duration::from_millis(8)),
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
    renderer.wait_scene_frame().ok();
    rt.touch(|s| game.retire(&mut s.editor, &mut renderer));
    compositor.retire(&renderer);
    drop(compositor);
    scene_view
        .destroy(&mut renderer)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
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

/// The Scene view's `CAMetalLayer`.
fn metal_layer(
    handle: stillkit::NativeHandle,
) -> anyhow::Result<std::ptr::NonNull<std::ffi::c_void>> {
    match handle {
        stillkit::NativeHandle::Metal(layer) => {
            std::ptr::NonNull::new(layer).context("the Scene layer is null")
        }
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!(
            "the studio's Scene view presents on macOS today; Windows and Linux use --legacy-ui until the Vulkan layer path lands"
        ),
    }
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
fn gizmo_state(s: &Studio) -> crate::shell::overlay::GizmoState {
    // While dragging, the gizmo draws at the previewed pose, not the committed one.
    let offset = match (&s.local.gizmo, &s.editor.gesture) {
        (Some(_), Some(g)) => g.world_delta(s.editor.state.selection.first().map(String::as_str)),
        _ => glam::Mat4::IDENTITY,
    };
    crate::shell::overlay::GizmoState {
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

/// The window's own controls: the system's on macOS, drawn ones elsewhere.
fn frame(surface: &Surface) -> state::Frame {
    let s = surface.scale();
    state::Frame {
        inset: surface.system_controls().map_or(0.0, |r| r.right / s),
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

/// Handles window events, runs a UI frame and presents it; whether the window closed.
/// While the game has the keyboard and mouse (`game`, once the renderer runs), its events
/// go to the player instead: Esc, leaving the window or a click outside the Game view give
/// them back; F5 to F8 stay the editor's Play controls.
fn pump(
    rt: &mut Runtime<Studio>,
    surface: &mut Surface,
    started: Instant,
    mut game: Option<&mut game::Game>,
) -> anyhow::Result<bool> {
    while let Some((id, event)) = next_event() {
        if id != surface.id() {
            continue;
        }
        if rt.state().local.game_captured
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
            Event::Close => return Ok(true),
            Event::Resized(w, h) => {
                rt.resize((w, h));
                let (size, frame) = (
                    (w as f32 / rt.scale(), h as f32 / rt.scale()),
                    frame(surface),
                );
                rt.update(|s| {
                    s.size = size;
                    s.frame = frame;
                });
            }
            Event::DpiChanged(dpi) => rt.set_scale(dpi as f32 / 96.0),
            Event::Key(vk) => {
                if FLY_KEYS.contains(&vk) && rt.state().local.flying {
                    rt.touch(|s| s.local.held.insert(vk));
                }
                rt.key_with(vk, modifiers());
            }
            Event::KeyUp(vk) => {
                rt.touch(|s| s.local.held.remove(&vk));
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Event::Char(c) => rt.type_text(c.encode_utf8(&mut [0; 4])),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Event::Preedit => {
                let (text, at) = stillkit::platform::current::ime::preedit();
                rt.preedit(&text, at);
            }
            Event::SettingChanged => rt.set_reduced_motion(reduced_motion()),
            #[cfg(any(windows, target_os = "macos"))]
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
    let painted = rt.frame(started.elapsed().saturating_sub(rt.now()));
    if let Some(key) = rt.touch(|s| s.local.focus.take()) {
        rt.focus(key);
    }
    if let Some(text) = rt.touch(|s| s.local.copy.take()) {
        stillkit::el::Clipboard::set(&mut SystemClipboard, &text);
    }
    if painted {
        rt.present(surface, (0, 0))?;
        chrome_parts(surface, rt);
        // Layout feedback the view reads next time: where splits and the Scene view are.
        let mut keys = dock::rect_keys(&rt.state().editor.state.layout);
        keys.push("scene/view".into());
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
        rt.touch(|s| s.local.rects = rects);
    }
    rt.sync_text_input(surface);
    surface.set_cursor(rt.cursor());
    Ok(false)
}
