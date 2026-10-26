//! The native Hub window: SDL3 through ara-platform, the editor's Vulkan UI
//! adapter (`ara-editor-gpu`) on a plain device (no scene renderer), and one
//! retained `Ui` reconciled from the view-model whenever it changes.
mod app;
mod frame;
mod images;
mod input;
pub(crate) mod os;
mod prefs;
#[cfg(feature = "rehearsal")]
mod rehearsal;
mod updates;
mod wake;

use crate::{
    capture,
    connection::Connection,
    model::{Effect, Hub, Prefs, engines},
};
use anyhow::Context as _;
pub use app::App;
use ara_editor_gpu::NativeUi;
use ara_gpu::vulkan::Context as Gpu;
use ara_platform::{Event, Present, Window};
use ara_ui::input::InputEvent;
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::Clipboard;
use input::NativeInput;
pub(crate) use input::{OsClipboard, deliver_edit};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Command-line options of `ara-hub`.
#[derive(Clone, Debug)]
pub struct Options {
    /// Windowless, compile-time-only release rehearsal driver.
    #[cfg(feature = "rehearsal")]
    pub rehearse_update: Option<PathBuf>,
    /// Internal, windowless installed-service check used by release smoke.
    pub service_check: Option<PathBuf>,
    /// Internal updater acknowledgment, written after the first rendered GUI frame.
    pub update_health: Option<PathBuf>,
    /// Initial window size in physical pixels.
    pub size: [u32; 2],
    /// Fixed UI scale (captures); the display's scale otherwise.
    pub dpi: Option<f32>,
    /// Write captures of every view to this folder, then exit.
    pub capture: Option<PathBuf>,
    /// User config directory; `ara_hub_service::config_dir()` otherwise.
    pub config: Option<PathBuf>,
    /// The `ara` CLI that starts the Hub service; the one beside `ara-hub` otherwise.
    pub ara: Option<PathBuf>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            #[cfg(feature = "rehearsal")]
            rehearse_update: None,
            service_check: None,
            update_health: None,
            size: [1440, 900],
            dpi: None,
            capture: None,
            config: None,
            ara: None,
        }
    }
}

/// Window, GPU device, UI adapter and swapchain, destroyed in a safe order.
struct Host {
    input: NativeInput,
    ui_gpu: NativeUi,
    present: Present,
    gpu: Gpu,
    window: Window,
    renderer: Renderer,
    dpi: f32,
    fixed_dpi: Option<f32>,
    physical: [u32; 2],
}
impl Host {
    fn new(options: &Options, light: bool) -> anyhow::Result<Self> {
        let window = Window::new("Ara Hub", options.size)?;
        frame::style(window.raw(), light);
        let input = NativeInput::attach(window.raw())?;
        let gpu = Gpu::new(&window.extensions()?)?;
        // SAFETY: `gpu` is declared after `ui_gpu` and `present` in Host, so it outlives
        // both; all queue access stays on this thread.
        let ui_gpu = unsafe { NativeUi::new(&gpu)? };
        // SAFETY: Host::drop waits for the device and destroys this before the window/device.
        let present = unsafe { Present::new(&gpu, &window)? };
        let dpi = options.dpi.unwrap_or_else(|| input.dpi());
        let mut host = Self {
            input,
            ui_gpu,
            present,
            gpu,
            physical: window.size()?,
            window,
            renderer: Renderer::default(),
            dpi,
            fixed_dpi: options.dpi,
        };
        host.ui_gpu.set_dpi(dpi)?;
        images::install(&mut host.renderer, dpi)?;
        Ok(host)
    }
    /// Logical size at the current scale.
    fn logical(&self) -> [f32; 2] {
        self.physical.map(|v| v as f32 / self.dpi)
    }
    /// Follow window resizes and monitor scale changes; true when the UI must relayout.
    fn track(&mut self) -> anyhow::Result<bool> {
        let physical = self.window.size()?;
        let dpi = self.fixed_dpi.unwrap_or_else(|| self.input.dpi());
        if physical == self.physical && dpi == self.dpi || physical.contains(&0) {
            return Ok(false);
        }
        // SAFETY: Wait before destroying a swapchain the presentation queue references.
        unsafe {
            self.gpu.device.device_wait_idle()?;
            self.present.destroy(&self.gpu);
            self.present = Present::new(&self.gpu, &self.window)?;
        }
        if dpi != self.dpi {
            images::install(&mut self.renderer, dpi)?;
        }
        self.physical = physical;
        self.dpi = dpi;
        self.ui_gpu.set_dpi(dpi)?;
        Ok(true)
    }
    fn render(&mut self, app: &App) -> anyhow::Result<()> {
        draw(&mut self.renderer, &mut self.ui_gpu, app)?;
        let (image, size) = self.ui_gpu.output()?;
        // SAFETY: The UI submission completed; the image is CopySource; owners are live.
        unsafe { self.present.display(&self.gpu, image, size) }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: Complete all submissions before destroying the swapchain; the UI adapter
        // and device then drop in field order (adapter first).
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            self.present.destroy(&self.gpu);
        }
    }
}

/// Render the app's display list into the adapter's current target.
fn draw(renderer: &mut Renderer, ui_gpu: &mut NativeUi, app: &App) -> anyhow::Result<()> {
    ui_gpu.select_target(0);
    renderer
        .render(
            ui_gpu,
            &app.ui.display,
            &app.text.assets,
            app.ui.size.map(|v| v.round() as u32),
            TextMode::Raster,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned))
        .unwrap_or_default()
}

/// Documents (or home) as the Create view's starting destination.
fn home() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let documents = home.join("Documents");
    if documents.is_dir() { documents } else { home }
}

fn load_prefs(config: &Path) -> Prefs {
    std::fs::read(config.join("hub-ui.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
/// Open the Hub window and run until it closes (or until captures are written).
pub fn run(options: Options) -> anyhow::Result<()> {
    let exe_dir = exe_dir();
    let config = match &options.config {
        Some(dir) => dir.clone(),
        None => ara_hub_service::config_dir().context("find the Ara config directory")?,
    };
    let config = std::path::absolute(config)?;
    ara_hub_service::validate_development_config(&config)?;
    if let Some(report) = &options.service_check {
        use ara_proto::Endpoint;
        let ara = updates::service(&exe_dir, options.ara.as_deref())?;
        let mut client =
            ara_hub_service::connect_or_start(&config, &ara).map_err(|e| anyhow::anyhow!("{e}"))?;
        let status = client
            .call("hub.status", serde_json::json!({}))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let templates = client
            .call("hub.templates", serde_json::json!({}))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        ara_update::write_json(
            report,
            &serde_json::json!({"executable":ara,"status":status,"templates":templates["templates"]}),
        )?;
        return Ok(());
    }
    #[cfg(feature = "rehearsal")]
    if std::env::var_os("ARA_RELEASE_TEST_API").is_some() {
        return rehearsal::run(&options, &exe_dir, &config);
    }
    let prefs = if options.capture.is_some() {
        Prefs::default()
    } else {
        load_prefs(&config)
    };
    let mut hub = Hub::new(prefs, home(), engines::installed(&exe_dir));
    let docs = os::docs(&exe_dir);
    if let Some(directory) = options.capture.clone() {
        let mut offscreen = Offscreen::new(options.size, options.dpi.unwrap_or(1.))?;
        let mut app = App::new(offscreen.logical(), docs).map_err(err)?;
        return capture::run(&mut offscreen, &mut app, &mut hub, &directory);
    }
    let mut host = Host::new(&options, hub.prefs.light)?;
    let ara = updates::service(&exe_dir, options.ara.as_deref())?;
    hub.releases.can_self_update = exe_dir.join("hub.json").is_file();
    let mut app = App::new(host.logical(), docs).map_err(err)?;
    interactive(
        &mut host,
        &mut app,
        &mut hub,
        &config,
        &ara,
        options.update_health.as_deref(),
    )
}

/// Captures without a window: a headless device, the same `NativeUi` adapter and
/// the same render path, read back from its target. Nothing appears on screen.
pub struct Offscreen {
    ui_gpu: NativeUi,
    renderer: Renderer,
    // Declared last so the adapter above drops first.
    _gpu: Gpu,
    physical: [u32; 2],
    dpi: f32,
}
impl Offscreen {
    fn new(physical: [u32; 2], dpi: f32) -> anyhow::Result<Self> {
        let gpu = Gpu::new(&[])?;
        // SAFETY: `_gpu` is a later field, so it outlives the adapter; single-threaded use.
        let mut ui_gpu = unsafe { NativeUi::new(&gpu)? };
        ui_gpu.set_dpi(dpi)?;
        let mut renderer = Renderer::default();
        images::install(&mut renderer, dpi)?;
        Ok(Self {
            ui_gpu,
            renderer,
            _gpu: gpu,
            physical,
            dpi,
        })
    }
    /// Logical size.
    pub fn logical(&self) -> [f32; 2] {
        self.physical.map(|v| v as f32 / self.dpi)
    }
    /// Physical extent.
    pub fn physical(&self) -> [u32; 2] {
        self.physical
    }
    /// Upload finished previews; true when any arrived.
    pub fn upload(&mut self, app: &mut App) -> bool {
        app.thumbnails.upload(&mut self.renderer)
    }
    /// Render and read back the actual GPU target.
    pub fn capture(&mut self, app: &App) -> anyhow::Result<image::RgbaImage> {
        draw(&mut self.renderer, &mut self.ui_gpu, app)?;
        self.ui_gpu.pixels()
    }
}

fn interactive(
    host: &mut Host,
    app: &mut App,
    hub: &mut Hub,
    config: &Path,
    ara: &Path,
    mut health: Option<&Path>,
) -> anyhow::Result<()> {
    let prefs = prefs::Writer::new(config.to_owned())?;
    let connection = Connection::start_with_wake(config.to_owned(), ara.to_owned(), wake::wake);
    let picks = os::picks();
    let (send, snippets) = std::sync::mpsc::channel();
    let snippet_ara = ara.to_owned();
    std::thread::spawn(move || {
        for snippet in os::install_snippets(&snippet_ara) {
            let _ = send.send(snippet);
            wake::wake();
        }
    });
    let mut saved = hub.prefs.clone();
    let mut dirty = false;
    let mut last_minute = 0;
    let mut last_check = std::time::Instant::now();
    hub.intent(crate::model::Intent::CheckUpdates);
    app.rebuild(hub, now_ms()).map_err(err)?;
    loop {
        dirty |= hub.tick(std::time::Instant::now());
        if last_check.elapsed() >= Duration::from_secs(4 * 3600) {
            hub.intent(crate::model::Intent::CheckUpdates);
            last_check = std::time::Instant::now();
        }
        let mut intents = Vec::new();
        let scale = host.input.points_per_logical(host.dpi);
        for event in host.window.poll()? {
            match event {
                Event::Quit => return Ok(()),
                Event::Resize(_) => dirty = true,
                Event::Key {
                    scancode, pressed, ..
                } => {
                    if let Some(actions) = input::recover(app.key(hub, scancode, pressed)) {
                        intents.extend(actions);
                    }
                }
                Event::MouseMotion { position, .. } => {
                    let _ = input::recover(app.dispatch_pointer(
                        hub,
                        InputEvent::PointerMove(position.map(|v| v * scale)),
                        now_ms(),
                    ));
                }
                Event::MouseButton { button: 1, pressed } => {
                    let at = app.pointer;
                    let event = if pressed {
                        InputEvent::PointerDown(at)
                    } else {
                        InputEvent::PointerUp(at)
                    };
                    let _ = input::recover(app.dispatch_pointer(hub, event, now_ms()));
                    // Clicking away from an inline rename saves it, as in Explorer.
                    if pressed
                        && hub.rename.is_some()
                        && hub.focus.is_none()
                        && app.focused() != Some("rename-field")
                    {
                        intents.push(crate::model::Intent::RenameCommit);
                    }
                }
                Event::MouseButton { .. } => {}
                Event::Wheel(delta) => {
                    if hub.menu.is_some() {
                        intents.push(crate::model::Intent::Menu(None));
                    }
                    let event = InputEvent::Wheel {
                        position: app.pointer,
                        delta: [-delta[0] * 48., -delta[1] * 48.],
                    };
                    let _ = input::recover(app.dispatch_pointer(hub, event, now_ms()));
                }
            }
            intents.extend(app.actions());
            if !intents.is_empty() {
                for intent in intents.drain(..) {
                    hub.intent(intent);
                }
                app.rebuild(hub, now_ms()).map_err(err)?;
            }
        }
        let _ = input::recover(host.input.pump(&mut app.ui, &mut app.input));
        intents.extend(
            host.input
                .take_drops()
                .into_iter()
                .map(crate::model::Intent::Register),
        );
        intents.extend(app.actions());
        for (pick, path) in picks.try_iter() {
            intents.push(crate::model::Intent::Picked(pick, path));
        }
        for (client, text) in snippets.try_iter() {
            hub.install.insert(client, text);
            dirty = true;
        }
        for event in connection.poll() {
            hub.event(event);
            dirty = true;
        }
        for intent in intents {
            hub.intent(intent);
            dirty = true;
        }
        connection.send(hub.take_calls());
        for effect in hub.take_effects() {
            match effect {
                Effect::ApplyUpdate {
                    archive,
                    manifest,
                    signature,
                } => match updates::launch(config, ara, archive, manifest, signature) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        hub.update_error(error.to_string());
                        dirty = true;
                    }
                },
                Effect::Pick(pick) => {
                    let start = match pick {
                        crate::model::Pick::Destination => {
                            Some(PathBuf::from(hub.create.destination.trim()))
                        }
                        _ => None,
                    };
                    os::pick_folder(host.window.raw(), pick, start.as_deref());
                }
                Effect::Reveal(path) => os::reveal(&path),
                Effect::Open(path) => os::open(&path),
                Effect::Copy(text) => {
                    let _ = input::recover(OsClipboard.write(&text));
                }
            }
        }
        if hub.prefs != saved {
            if hub.prefs.light != saved.light {
                frame::style(host.window.raw(), hub.prefs.light);
            }
            prefs.save(hub.prefs.clone());
            saved = hub.prefs.clone();
        }
        dirty |= app.thumbnails.upload(&mut host.renderer);
        if host.physical.contains(&0) || host.window.size()?.contains(&0) {
            // SAFETY: Main-thread SDL pump; input and worker events wake a minimized Hub.
            unsafe {
                sdl3_sys::events::SDL_WaitEventTimeout(std::ptr::null_mut(), 1000);
            }
            host.physical = host.window.size()?;
            continue;
        }
        if host.track()? {
            app.size = host.logical();
            dirty = true;
        }
        let now = now_ms();
        if now / 60_000 != last_minute {
            last_minute = now / 60_000;
            dirty = true;
        }
        if dirty {
            app.rebuild(hub, now).map_err(err)?;
            dirty = false;
        }
        if app.frame().map_err(err)? {
            host.input.sync_text_input(&app.ui, &app.input, host.dpi)?;
            host.render(app)?;
            if let Some(marker) = health.take() {
                std::fs::write(marker, b"ready")?;
            }
        }
        // Worker results push an SDL wake event. The only idle deadline is the
        // next relative-time label or informational toast expiration.
        let mut wait_ms = 60_000 - now_ms() % 60_000;
        if let Some(deadline) = hub.notice_deadline() {
            wait_ms = wait_ms.min(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis() as u64,
            );
        }
        wait_ms = wait_ms.min(
            (Duration::from_secs(4 * 3600).saturating_sub(last_check.elapsed())).as_millis() as u64,
        );
        // SAFETY: Main-thread SDL pump; a null event leaves input queued for poll().
        unsafe {
            sdl3_sys::events::SDL_WaitEventTimeout(std::ptr::null_mut(), wait_ms.max(1) as i32);
        }
    }
}

fn err(error: Box<dyn std::error::Error>) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

/// Show a fatal startup error to someone who launched the Hub from the Start menu.
pub fn fatal(message: &str) {
    let text = std::ffi::CString::new(message.replace('\0', " ")).unwrap_or_default();
    // SAFETY: SDL copies both strings; a null parent window is allowed.
    unsafe {
        sdl3_sys::messagebox::SDL_ShowSimpleMessageBox(
            sdl3_sys::messagebox::SDL_MESSAGEBOX_ERROR,
            c"Ara Hub".as_ptr(),
            text.as_ptr(),
            std::ptr::null_mut(),
        );
    }
}
