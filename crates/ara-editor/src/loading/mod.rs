//! The loading window. Before the main window appears, a small undecorated window in
//! the Hub's visual language shows the project's imports from
//! [`ara_play_host::ImportProgress`]: the source that finished last, N of M, a bar and
//! the elapsed time, with failed imports listed inline with their errors. The editor
//! opens once every import has settled, so it never opens on models popping in. A few
//! seconds in, "Open now" opens it at once (imports carry on in the editor); failures
//! hold the window on a review until "Open project".
//!
//! `app::run` calls [`run`] after opening the project and before creating its main
//! window; meanwhile the authority ticks (imports progress there) and EP clients are
//! served. A project whose imports settle within [`SHOW_AFTER`] opens with no window.
//! The window then shows "Opening …" until the main window has presented its first
//! frame ([`Splash::hand_over`]). Bounded runs skip it; `--verify-loading` captures a
//! real run and every state offscreen ([`run`] with a capture folder).
mod captures;
mod view;

use crate::gpu::UiDevice as Gpu;
use crate::{Editor, transport::Server};
use anyhow::Context as _;
use ara_editor_gpu::NativeUi;
use ara_platform::{Event, Present, Window};
use ara_play_host::{ImportProgress, PublishedAssets};
use ara_ui::{Ui, input::InputState, source::stable_id, theme::EditorTheme};
use ara_ui_render::{
    atlas::TextMode,
    gpu::{Image, Renderer},
};
use ara_ui_text::TextSystem;
use captures::Captures;
use std::{
    cell::Cell,
    collections::BTreeSet,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) use view::{Imports, Stage, Status};

/// The window's stylesheet (Hub tokens and controls).
pub(crate) const CSS: &str = include_str!("loading.css");
/// Imports that settle sooner open the editor with no loading window.
const SHOW_AFTER: Duration = Duration::from_millis(400);
/// When "Open now" appears.
const SKIP_AFTER: Duration = Duration::from_secs(3);
/// A pipeline that has not reported by then is not coming (no import worker): open.
const GIVE_UP: Duration = Duration::from_secs(12);

/// How the loading phase ended.
pub(crate) enum Outcome {
    /// Open the editor. A splash that was shown stays up, saying "Opening …", until
    /// [`Splash::hand_over`].
    Open(Option<Box<Splash>>),
    /// The window was closed: quit without opening.
    Quit,
}

/// Run the loading phase for the open project: tick the authority and serve EP
/// clients until its imports settle (or the user opens now or closes the window).
/// With `capture`, nothing appears on screen: the window renders offscreen and its
/// milestones and every state are written there as PNGs, in both themes.
pub(crate) fn run(
    editor: &mut Editor,
    server: &mut Server,
    capture: Option<(&Path, f32)>,
) -> anyhow::Result<Outcome> {
    let started = Instant::now();
    let mut tracker = Tracker::new(editor)?;
    // A warm cache or a project without sources: no window at all.
    while capture.is_none() && started.elapsed() < SHOW_AFTER {
        server.pump(editor);
        editor.tick();
        tracker.poll();
        if tracker.settled_clean() {
            return Ok(Outcome::Open(None));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let root = editor.host.root();
    let project = crate::shell::project_name(editor);
    let folder = dunce::simplified(root).display().to_string();
    let light = editor.state.light;
    let mut splash = match Splash::new(&project, light, capture) {
        Ok(splash) => splash,
        // No window or device for it: open the editor as it is (it shows imports too).
        Err(error) if capture.is_none() => {
            eprintln!("ara-editor: no loading window ({error:#}); opening now");
            return Ok(Outcome::Open(None));
        }
        Err(error) => return Err(error),
    };
    let mut captures = capture.map(|(dir, _)| Captures::new(dir));
    let mut status = Status {
        project,
        folder,
        light,
        ..Default::default()
    };
    loop {
        let frame = Instant::now();
        server.pump(editor);
        editor.tick();
        tracker.poll();
        status.elapsed = started.elapsed().as_secs();
        status.skip = started.elapsed() >= SKIP_AFTER;
        status.imports = tracker.imports();
        status.latest = tracker.latest.clone();
        // One sweep of Preparing's bar every 1.6 s.
        status.sweep = if status.imports.is_none() {
            (started.elapsed().as_secs_f32() / 1.6).fract()
        } else {
            0.
        };
        if status.imports.is_none() && started.elapsed() >= GIVE_UP {
            status.opening = true;
        }
        let mut open = matches!(status.stage(), Stage::Opening);
        for event in splash.window.poll()? {
            match event {
                Event::Quit => return Ok(Outcome::Quit),
                // Enter takes the footer's button.
                Event::Key {
                    scancode: 40 | 88,
                    pressed: true,
                    ..
                } if status.button().is_some() => open = true,
                Event::MouseMotion { position, .. } => {
                    splash.pointer(ara_ui::input::InputEvent::PointerMove(position))?
                }
                Event::MouseButton { button: 1, pressed } => {
                    let at = splash.at;
                    splash.pointer(if pressed {
                        ara_ui::input::InputEvent::PointerDown(at)
                    } else {
                        ara_ui::input::InputEvent::PointerUp(at)
                    })?;
                    open |= splash
                        .ui
                        .drain_actions()
                        .any(|a| a.node == stable_id("loading-open"));
                }
                _ => {}
            }
        }
        splash.show(&status)?;
        if let Some(captures) = &mut captures {
            captures.milestone(&mut splash, &status)?;
            // Captures never wait for a person: a captured review opens.
            open |= status.stage() == Stage::Review;
        }
        if open {
            // The last frame until the main window takes over.
            status.opening = true;
            splash.show(&status)?;
            if let Some(captures) = &mut captures {
                captures.milestone(&mut splash, &status)?;
                captures.states(&mut splash, &status)?;
                return Ok(Outcome::Quit);
            }
            return Ok(Outcome::Open(Some(Box::new(splash))));
        }
        if let Some(rest) = Duration::from_millis(16).checked_sub(frame.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

/// Follows the import feed: the latest progress, the source that finished last and
/// asset IDs as project paths.
struct Tracker {
    reader: Option<PublishedAssets>,
    revision: u64,
    progress: Option<Arc<ImportProgress>>,
    /// The source that finished most recently.
    latest: Option<String>,
    /// Asset ID → source path (from the import documents' paths).
    paths: std::collections::BTreeMap<String, String>,
}
impl Tracker {
    fn new(editor: &Editor) -> anyhow::Result<Self> {
        let paths = editor
            .host
            .store()
            .workspace()
            .docs()
            .filter(|(_, d)| d.kind == "import")
            .map(|(path, d)| {
                let source = path.strip_suffix(".import.json").unwrap_or(path);
                (d.id.to_string(), source.to_owned())
            })
            .collect();
        Ok(Self {
            reader: PublishedAssets::for_project(editor.host.root()).ok(),
            revision: 0,
            progress: None,
            latest: None,
            paths,
        })
    }
    /// Read new progress, noting a source that left `pending` without failing.
    fn poll(&mut self) {
        let Some(reader) = &self.reader else {
            return;
        };
        let revision = reader.progress_revision();
        if revision == self.revision {
            return;
        }
        let Some(progress) = reader.progress() else {
            return;
        };
        self.revision = revision;
        if let Some(before) = &self.progress {
            let finished: BTreeSet<&String> = before
                .pending
                .difference(&progress.pending)
                .filter(|id| !progress.failed.contains_key(*id))
                .collect();
            if let Some(id) = finished.last() {
                self.latest = Some(self.path(id));
            }
        }
        self.progress = Some(progress);
    }
    fn path(&self, id: &str) -> String {
        self.paths.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }
    /// Every import settled with no failures.
    fn settled_clean(&self) -> bool {
        self.progress
            .as_ref()
            .is_some_and(|p| p.pending.is_empty() && p.failed.is_empty())
    }
    fn imports(&self) -> Option<Imports> {
        let p = self.progress.as_ref()?;
        let mut failed: Vec<(String, String)> = p
            .failed
            .iter()
            .map(|(id, error)| (self.path(id), error.clone()))
            .collect();
        failed.sort();
        Some(Imports {
            done: p.done,
            total: p.total,
            pending: p.pending.len(),
            failed,
        })
    }
}

/// The loading window: an undecorated, centred SDL window with its own device and
/// the editor's Vulkan UI adapter, one retained `Ui` reconciled from [`Status`].
/// Fields drop in order: adapter and swapchain before the device, the window last.
pub(crate) struct Splash {
    ui_gpu: NativeUi,
    present: Option<Present>,
    gpu: Gpu,
    window: Window,
    renderer: Renderer,
    ui: Ui,
    text: TextSystem,
    input: InputState,
    dpi: f32,
    /// Pointer in logical pixels.
    at: [f32; 2],
    light: bool,
    markup: String,
    /// The footer button's rect in window points: the one place that is not a drag
    /// handle (SDL's hit test reads it).
    button: Box<Cell<Option<[f32; 4]>>>,
    /// Offscreen (captures): never shown or presented.
    offscreen: bool,
}

impl Splash {
    /// An opening card while the main renderer's cold PSOs compile off-thread.
    pub(crate) fn opening(status: &Status) -> anyhow::Result<Box<Self>> {
        let mut splash = Box::new(Self::new(&status.project, status.light, None)?);
        splash.show(status)?;
        Ok(splash)
    }

    /// Pump and redraw during renderer preparation; closing cancels the open.
    pub(crate) fn opening_frame(&mut self, status: &Status) -> anyhow::Result<bool> {
        for event in self.window.poll()? {
            if matches!(event, Event::Quit) {
                return Ok(false);
            }
        }
        self.show(status)?;
        Ok(true)
    }

    fn new(project: &str, light: bool, capture: Option<(&Path, f32)>) -> anyhow::Result<Self> {
        let offscreen = capture.is_some();
        // Its own title: the main window is found by its title (`Native::attach`).
        let title = format!("Opening {project} | Ara");
        let window = Window::hidden(&title, [view::WIDTH as u32, view::BASE_HEIGHT as u32])?;
        // Native icons are cosmetic; Wayland may not support them.
        let _ = window.set_icon_png(include_bytes!(
            "../../../../assets/brand/icons/engine/ara-engine-64.png"
        ));
        let dpi = capture.map_or_else(|| window.display_scale(), |(_, dpi)| dpi);
        let size = [view::WIDTH, view::BASE_HEIGHT];
        let gpu = crate::gpu::open_presentation(&window.extensions()?)?;
        // SAFETY: `gpu` is a later field than `ui_gpu`, so it outlives the adapter; the
        // swapchain is destroyed in Drop before the device. All use stays on this thread.
        let mut ui_gpu = unsafe { NativeUi::new(&gpu)? };
        ui_gpu.set_dpi(dpi)?;
        let mut renderer = Renderer::default();
        crate::shell::icons::install(&mut renderer, dpi).map_err(|e| anyhow::anyhow!("{e}"))?;
        install_engine_icon(&mut renderer, dpi)?;
        let mut splash = Self {
            ui_gpu,
            present: None,
            gpu,
            window,
            renderer,
            ui: Ui::editor("<ui id='loading' />", CSS, size).map_err(|e| anyhow::anyhow!("{e}"))?,
            text: crate::shell::text_system().map_err(|e| anyhow::anyhow!("{e}"))?,
            input: InputState::default(),
            dpi,
            at: [-1.; 2],
            light: false,
            markup: String::new(),
            button: Box::new(Cell::new(None)),
            offscreen,
        };
        splash.resize(size, true)?;
        if !offscreen {
            frame(&splash.window, light, &splash.button);
            // SAFETY: Destroyed in Drop before the device.
            splash.present =
                Some(unsafe { splash.ui_gpu.create_present(&splash.gpu, &splash.window)? });
            // SAFETY: Live window on its owning (main) thread.
            unsafe {
                sdl3_sys::video::SDL_ShowWindow(splash.window.raw());
            }
        }
        Ok(splash)
    }

    /// Size the window to `size` logical pixels; `center` places it on the pointer's
    /// display (only when it opens: a window the user dragged stays put).
    fn resize(&mut self, size: [f32; 2], center: bool) -> anyhow::Result<()> {
        let physical = size.map(|v| (v * self.dpi).round() as u32);
        self.window.set_size(physical)?;
        let pointer = Window::global_mouse();
        let displays = if center {
            Window::displays()
        } else {
            Vec::new()
        };
        if let Some(d) = displays
            .iter()
            .find(|d| {
                pointer[0] >= d[0] as f32
                    && pointer[1] >= d[1] as f32
                    && pointer[0] < (d[0] + d[2]) as f32
                    && pointer[1] < (d[1] + d[3]) as f32
            })
            .or(displays.first())
        {
            self.window.set_position([
                d[0] + (d[2] - physical[0] as i32) / 2,
                d[1] + (d[3] - physical[1] as i32) * 2 / 5,
            ])?;
        }
        if size != self.ui.size {
            self.ui.resize(size).map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(present) = &mut self.present {
                // SAFETY: Wait before destroying a swapchain the queue may reference.
                unsafe {
                    self.ui_gpu.wait_idle(&self.gpu)?;
                    self.ui_gpu.retire_present(&self.gpu, present);
                    *present = self.ui_gpu.create_present(&self.gpu, &self.window)?;
                }
            }
        }
        Ok(())
    }

    /// Route a pointer event (window points) into the UI.
    fn pointer(&mut self, event: ara_ui::input::InputEvent) -> anyhow::Result<()> {
        use ara_ui::input::InputEvent::*;
        // SAFETY: Live main-thread SDL window.
        let density = unsafe { sdl3_sys::video::SDL_GetWindowPixelDensity(self.window.raw()) };
        let scale = density.max(0.25) / self.dpi;
        let event = match event {
            PointerMove(p) => PointerMove(p.map(|v| v * scale)),
            other => other,
        };
        if let PointerMove(p) = event {
            self.at = p;
        }
        self.input
            .event(&mut self.ui, event, &mut crate::native::OsClipboard)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Reconcile `status` and draw it (present, or keep the target for a capture).
    fn show(&mut self, status: &Status) -> anyhow::Result<()> {
        let size = [view::WIDTH, status.height()];
        if size != self.ui.size {
            self.resize(size, false)?;
        }
        if self.light != status.light {
            self.ui
                .switch_theme(if status.light {
                    EditorTheme::Light
                } else {
                    EditorTheme::Dark
                })
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            self.light = status.light;
            self.markup.clear();
        }
        let markup = view::markup(status, &mut Widths(&mut self.text));
        let changed = markup != self.markup;
        if changed {
            self.ui
                .reconcile(&markup)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            self.markup = markup;
        }
        let work = self
            .ui
            .frame(&mut self.text)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if !changed && work.paint_nodes == 0 && work.layout_passes == 0 && !self.offscreen {
            return Ok(());
        }
        // The footer button is the window's only control; the rest drags it.
        let points = self.dpi
            // SAFETY: Live main-thread SDL window.
            / unsafe { sdl3_sys::video::SDL_GetWindowPixelDensity(self.window.raw()) }.max(0.25);
        self.button.set(self.ui.find("loading-open").map(|i| {
            let r = self.ui.nodes[i].rect;
            [r.x * points, r.y * points, r.w * points, r.h * points]
        }));
        self.ui_gpu.select_target(0);
        self.renderer
            .render(
                &mut self.ui_gpu,
                &self.ui.display,
                &self.text.assets,
                self.ui.size.map(|v| v.round() as u32),
                TextMode::Raster,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(present) = &mut self.present {
            // SAFETY: The UI submission completed; the image is CopySource; owners are live.
            unsafe { self.ui_gpu.present(&self.gpu, present)? };
        }
        Ok(())
    }

    /// The main window has presented its first frame: show it, then close the splash.
    pub(crate) fn hand_over(self: Box<Self>, main: &Window) {
        // SAFETY: Both windows are live on this (main) thread.
        unsafe {
            sdl3_sys::video::SDL_ShowWindow(main.raw());
            sdl3_sys::video::SDL_RaiseWindow(main.raw());
        }
        drop(self);
    }
}
impl Drop for Splash {
    fn drop(&mut self) {
        // SAFETY: The hit test's data is the boxed button rect, freed after this; the
        // swapchain goes before the device (adapter, then device, drop in field order).
        unsafe {
            sdl3_sys::video::SDL_SetWindowHitTest(self.window.raw(), None, std::ptr::null_mut());
            let _ = self.ui_gpu.wait_idle(&self.gpu);
            if let Some(present) = &mut self.present {
                self.ui_gpu.retire_present(&self.gpu, present);
            }
        }
    }
}

/// [`view::Measure`] over the window's text system (its shaping cache keeps the
/// per-frame refits cheap).
struct Widths<'a>(&'a mut TextSystem);
impl view::Measure for Widths<'_> {
    fn width(&mut self, text: &str, size: f32) -> f32 {
        let mut line = ara_ui_text::Text::new(text.to_owned());
        self.0.measure(&mut line, None, size).width
    }
}

/// Undecorated, fixed-size, rounded (Windows 11) and draggable everywhere but the
/// footer button, in the theme's border color.
fn frame(window: &Window, light: bool, button: &Cell<Option<[f32; 4]>>) {
    use sdl3_sys::video::*;
    unsafe extern "C" fn hit(
        _: *mut SDL_Window,
        area: *const sdl3_sys::rect::SDL_Point,
        data: *mut std::ffi::c_void,
    ) -> SDL_HitTestResult {
        // SAFETY: `data` is the splash's boxed button rect, unset before it is freed;
        // SDL passes a valid point on the window's thread.
        let (button, p) = unsafe { (&*(data as *const Cell<Option<[f32; 4]>>), *area) };
        let (x, y) = (p.x as f32, p.y as f32);
        match button.get() {
            Some([bx, by, w, h]) if x >= bx && y >= by && x < bx + w && y < by + h => {
                SDL_HITTEST_NORMAL
            }
            _ => SDL_HITTEST_DRAGGABLE,
        }
    }
    // SAFETY: Live main-thread window; the hit-test data outlives the window's use of
    // it (Splash::drop unsets it first).
    unsafe {
        SDL_SetWindowBordered(window.raw(), false);
        SDL_SetWindowResizable(window.raw(), false);
        SDL_SetWindowHitTest(
            window.raw(),
            Some(hit),
            (button as *const Cell<Option<[f32; 4]>>).cast_mut().cast(),
        );
    }
    #[cfg(windows)]
    rounded(window, light);
    #[cfg(not(windows))]
    let _ = light;
}

/// Windows 11 rounds the borderless window and draws the theme's hairline border.
#[cfg(windows)]
fn rounded(window: &Window, light: bool) {
    use sdl3_sys::{properties::SDL_GetPointerProperty, video::*};
    use windows::Win32::{Foundation::HWND, Graphics::Dwm::*};
    let tokens = if light {
        EditorTheme::Light
    } else {
        EditorTheme::Dark
    }
    .tokens()
    .unwrap_or_default();
    // COLORREF is 0x00BBGGRR.
    let border = tokens
        .get("--separator-strong")
        .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
        .map_or(0x002e2e2e, |rgb| {
            ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
        });
    // SAFETY: The SDL window is live on this thread; DWM reads the values only for the
    // duration of each call. Older Windows builds ignore these attributes.
    unsafe {
        let hwnd = SDL_GetPointerProperty(
            SDL_GetWindowProperties(window.raw()),
            SDL_PROP_WINDOW_WIN32_HWND_POINTER,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        let hwnd = HWND(hwnd);
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const round).cast(),
            std::mem::size_of_val(&round) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const border).cast(),
            std::mem::size_of::<u32>() as u32,
        );
    }
}

/// The engine icon for both themes, at 2× the device scale (30 × 30 logical).
fn install_engine_icon(renderer: &mut Renderer, dpi: f32) -> anyhow::Result<()> {
    let scale = dpi.max(1.) * 2.;
    let size = [(30. * scale).round() as u32; 2];
    let svg = include_str!("../../../../assets/brand/icons/engine/ara-engine.svg");
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())?;
    let mut pixels =
        resvg::tiny_skia::Pixmap::new(size[0], size[1]).context("engine icon extent")?;
    let fit = (size[0] as f32 / tree.size().width()).min(size[1] as f32 / tree.size().height());
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(fit, fit),
        &mut pixels.as_mut(),
    );
    // tiny-skia stores premultiplied pixels; UI images are straight alpha.
    let mut rgba = pixels.take();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            for n in 0..3 {
                pixel[n] = ((u32::from(pixel[n]) * 255) / u32::from(pixel[3])).min(255) as u8;
            }
        }
    }
    renderer
        .image(
            stable_id("loading-engine") as u32 & 0x7fff_ffff,
            Image {
                width: size[0],
                height: size[1],
                rgba,
            },
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
