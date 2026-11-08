//! The native Hub window (D31): one stillkit app surface, CPU-drawn and redrawn only where
//! something changed, hosting a runtime over [`HubApp`]. The service connection, pictures,
//! snippets and preference writes run on workers and wake the loop through stillkit's
//! [`Waker`](stillkit::Waker); with nothing to do the loop sleeps with no timer but the next
//! relative-time label, toast expiry or update check.
pub(crate) mod os;
mod prefs;
#[cfg(feature = "rehearsal")]
mod rehearsal;
mod startup;
mod updates;

use crate::{
    capture::{self, Capture},
    model::{Effect, Hub, Intent, Pick, Prefs, View, engines},
    ui::{self, HubApp},
};
use anyhow::Context as _;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use stillkit::el::{Clipboard as _, Runtime, SystemClipboard};
use stillkit::platform::current::host::{modifiers, reduced_motion, wait, wake_at};
use stillkit::{Caption, Chrome, Event, Kind, Role, Surface, next_event};

/// Command-line options of `ara-hub`.
#[derive(Clone, Debug)]
pub struct Options {
    /// Start in the existing New project form; otherwise show Projects.
    pub new_project: bool,
    /// Measure the live installed startup offscreen and write frames/timings here.
    pub startup_probe: Option<PathBuf>,
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
            new_project: false,
            startup_probe: None,
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
    if let Some(output) = &options.startup_probe {
        return startup::run(&options, output, &exe_dir, &config);
    }
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
    if options.rehearse_update.is_some()
        || options.update_health.is_some()
        || std::env::var_os("ARA_RELEASE_TEST_API").is_some()
    {
        return rehearsal::run(&options, &exe_dir, &config);
    }
    let prefs = if options.capture.is_some() {
        Prefs::default()
    } else {
        load_prefs(&config)
    };
    let mut hub = Hub::new(prefs, home(), engines::installed(&exe_dir));
    if options.new_project {
        hub.intent(Intent::Navigate(View::Create));
    }
    let docs = os::docs(&exe_dir);
    if let Some(directory) = options.capture.clone() {
        let mut cap = Capture::new(options.size, options.dpi.unwrap_or(1.0), docs)?;
        return capture::run(&mut cap, &mut hub, &directory);
    }
    hub.releases.can_self_update = exe_dir.join("hub.json").is_file();
    window(&options, hub, docs, &config, &exe_dir)
}

/// The window's own controls: the system's on macOS, drawn ones elsewhere.
fn frame(surface: &Surface) -> ui::Frame {
    let s = surface.scale();
    ui::Frame {
        inset: surface.system_controls().map_or(0.0, |r| r.right / s),
        controls: (!cfg!(target_os = "macos")).then(|| surface.is_maximized()),
    }
}

/// Marks the title bar's free space as the caption (it drags the window) and the drawn
/// controls as the platform's, after each layout.
fn chrome(surface: &Surface, rt: &Runtime<HubApp>) {
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

fn window(
    options: &Options,
    hub: Hub,
    docs: Vec<ui::Doc>,
    config: &Path,
    exe_dir: &Path,
) -> anyhow::Result<()> {
    // Window classes and the Wayland app_id read "ara-hub".
    let _ = stillkit::set_identity(stillkit::Identity {
        prefix: "ara-hub",
        desktop_classes: &[],
    });
    let mut surface = Surface::new(Kind::App { min: (960, 600) }, "Ara Hub", Role::Window)?;
    surface.set_caption(Caption::Compact);
    let scale = options.dpi.unwrap_or_else(|| surface.scale());
    // 1280 x 820 design px unless --size asked for device px; never larger than the screen.
    let design = if options.size == Options::default().size {
        (1280.0, 820.0)
    } else {
        (
            options.size[0] as f32 / scale,
            options.size[1] as f32 / scale,
        )
    };
    let (sw, sh) = stillkit::platform::current::host::screen_size();
    let w = ((design.0 * scale) as i32).min(sw - 40);
    let h = ((design.1 * scale) as i32).min(sh - 80);
    surface.place(((sw - w) / 2, (sh - h) / 2, w, h));
    let waker = stillkit::waker();
    let wake = {
        let waker = waker.clone();
        move || waker.wake()
    };
    let connection = updates::connect(
        exe_dir.to_owned(),
        config.to_owned(),
        options.ara.clone(),
        wake.clone(),
    );
    let size = surface.client_size();
    let mut app = HubApp::new(hub, docs, size.0 as f32 / scale);
    app.now_ms = now_ms();
    app.frame = frame(&surface);
    let theme = app.look.theme.clone();
    let background = app.look.c.background.0;
    let mut rt = Runtime::new(app, ui::view, theme, size, scale)?;
    rt.set_background(Some(background));
    rt.set_clipboard(SystemClipboard);
    rt.set_reduced_motion(reduced_motion());
    surface.set_background(background);
    rt.update(|s| s.intent(Intent::CheckUpdates));
    rt.frame(Duration::ZERO);
    rt.present(&mut surface, (0, 0))?;
    chrome(&surface, &rt);
    surface.show_active(false);
    let prefs = prefs::Writer::new(config.to_owned())?;
    let mut saved = rt.state().hub.prefs.clone();
    let started = Instant::now();
    let mut last_check = Instant::now();
    let mut minute = now_ms() / 60_000;
    let mut health = options.update_health.clone();
    let mut snippets: Option<std::sync::mpsc::Receiver<(crate::model::AgentClient, String)>> = None;
    let mut ara: Option<PathBuf> = None;
    let mut cursor = stillkit::Cursor::Arrow;
    let mut frames: Option<stillkit::AnimationFrames> = None;
    loop {
        wait();
        while let Some((id, event)) = next_event() {
            if id != surface.id() {
                continue;
            }
            match event {
                Event::Close => return Ok(()),
                Event::Resized(w, h) => {
                    rt.resize((w, h));
                    let (width, frame) = (w as f32 / rt.scale(), frame(&surface));
                    rt.update(|s| {
                        s.width = width;
                        s.frame = frame;
                    });
                }
                Event::DpiChanged(dpi) => rt.set_scale(dpi as f32 / 96.0),
                Event::Key(vk) => rt.key_with(vk, modifiers()),
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
        // Service events, worker results and the clock.
        let events = connection.poll();
        let mut changed = !events.is_empty();
        if !events.is_empty() {
            rt.update(|s| {
                for event in events {
                    if let crate::connection::Event::Prepared { executable } = &event {
                        ara = Some(executable.clone());
                    }
                    s.hub.event(event);
                }
            });
        }
        if let Some(r) = &snippets {
            let got: Vec<_> = r.try_iter().collect();
            if !got.is_empty() {
                rt.update(|s| s.hub.install.extend(got));
            }
        }
        let now = now_ms();
        let ticked = rt.touch(|s| s.hub.tick(Instant::now()));
        if ticked || now / 60_000 != minute {
            minute = now / 60_000;
            rt.update(|s| s.now_ms = now);
        }
        if last_check.elapsed() >= Duration::from_secs(4 * 3600) {
            last_check = Instant::now();
            rt.update(|s| s.intent(Intent::CheckUpdates));
        }
        if rt.touch(|s| s.images.receive()) {
            rt.update(|_| {});
        }
        // Frame, then what the frame's handlers asked of the world.
        rt.sync_text_input(&surface);
        if rt.frame(started.elapsed().saturating_sub(rt.now())) {
            rt.present(&mut surface, (0, 0))?;
            chrome(&surface, &rt);
            if let Some(marker) = health.take() {
                std::fs::write(marker, b"ready")?;
            }
        }
        rt.sync_text_input(&surface);
        let (calls, effects) = rt.touch(|s| (s.hub.take_calls(), s.hub.take_effects()));
        connection.send(calls);
        for effect in effects {
            changed = true;
            match effect {
                Effect::ApplyUpdate {
                    archive,
                    manifest,
                    signature,
                } => {
                    match ara
                        .as_deref()
                        .context("The Hub service is still connecting; retry once connected")
                        .and_then(|ara| updates::launch(config, ara, archive, manifest, signature))
                    {
                        Ok(()) => return Ok(()),
                        Err(error) => rt.update(|s| s.hub.update_error(error.to_string())),
                    }
                }
                Effect::Pick(pick) => {
                    let start = (pick == Pick::Destination)
                        .then(|| PathBuf::from(rt.state().hub.create.destination.trim()));
                    if let Some(path) = os::pick_folder(pick, start.as_deref()) {
                        rt.update(|s| s.intent(Intent::Picked(pick, path)));
                    }
                }
                Effect::Reveal(path) => os::reveal(&path),
                Effect::Open(path) => os::open(&path),
                Effect::Copy(text) => SystemClipboard.set(&text),
            }
        }
        // Agent setup snippets, printed by the real CLI, once Agents is opened.
        if snippets.is_none()
            && rt.state().hub.view == View::Agents
            && let Some(cli) = ara.clone()
        {
            let (send, receive) = std::sync::mpsc::channel();
            let wake = wake.clone();
            std::thread::spawn(move || {
                for snippet in os::install_snippets(&cli) {
                    let _ = send.send(snippet);
                    wake();
                }
            });
            snippets = Some(receive);
        }
        // Preferences and the theme.
        let current = rt.state().hub.prefs.clone();
        if current != saved {
            prefs.save(current.clone());
            saved = current;
            if rt.touch(|s| {
                let switched = s.sync_look();
                if switched {
                    s.images.keep_scheme(s.look.dark);
                }
                switched
            }) {
                let (theme, background) = (
                    rt.state().look.theme.clone(),
                    rt.state().look.c.background.0,
                );
                rt.set_theme(theme);
                rt.set_background(Some(background));
                surface.set_background(background);
                rt.update(|_| {});
            }
        }
        if changed {
            rt.update(|_| {});
        }
        // Pictures the views asked for.
        rt.touch(|s| {
            let look = s.look.clone();
            s.images.start(&look, wake.clone());
        });
        let shape = rt.cursor();
        if shape != cursor {
            surface.set_cursor(shape);
            cursor = shape;
        }
        // Display-rate frames only while something moves; none once it settles.
        if rt.animating() {
            frames.get_or_insert_with(|| surface.animation_frames(Duration::from_secs(1)));
        } else {
            frames = None;
        }
        // The next deadline: the runtime's timers, a toast's expiry, the next minute's
        // relative times, the next update check.
        let mut next = Instant::now() + Duration::from_millis(60_000 - now_ms() % 60_000);
        if let Some(at) = rt.wake_at().map(|at| started + at) {
            next = next.min(at);
        }
        if let Some(deadline) = rt.state().hub.notice_deadline() {
            next = next.min(deadline);
        }
        next = next.min(last_check + Duration::from_secs(4 * 3600));
        wake_at(Some(next));
    }
}

/// Show a fatal startup error to someone who launched the Hub from the Start menu.
pub fn fatal(message: &str) {
    stillkit::dialog::alert("Ara Hub could not start", message);
}
