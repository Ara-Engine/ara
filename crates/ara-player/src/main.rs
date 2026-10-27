//! `ara-player`: the S5 play process. Spawned by the edit-world owner (the `ara`
//! daemon or the editor) into a pre-spawned, single-use pool. It initializes, connects
//! back over localhost WebSocket with the session token and idles at zero CPU until
//! `play.start` assigns a role and a shared-memory snapshot; then it runs that world
//! (windowed through SDL3, embedded in the owner through a shared GPU surface after
//! `play.embed`, or headless) and exits on `play.stop`.
extern crate glam34 as glam;
mod crash;
mod device;
mod embed;
mod game_app;
mod game_ui;
#[cfg(all(test, windows))]
mod icon_tests;
mod input;
mod net;
mod packed;
mod pending_models;
mod standalone;
mod world;

use anyhow::{Context, Result, anyhow};
use ara_play::{
    HELLO, Hello, PROTOCOL, STATUS, Start, Status, TOKEN_ENV,
    link::{Link as SocketLink, Message, Pump as Link, error},
    method,
    shm::Section,
};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, QualityChoice, RenderOptions, two_d::Presenter2d};
use clap::Parser;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use world::{PendingPatch, World};

/// Command line. The session token comes from `ARA_PLAY_TOKEN`, never an argument.
#[derive(Parser)]
#[command(name = "ara-player", about = "Ara game player and editor Play process")]
struct Args {
    #[command(flatten)]
    game: game_app::Options,
    /// Owner's live-link port on 127.0.0.1.
    #[arg(long)]
    owner: Option<u16>,
    /// Pool slot name, for logs and window titles.
    #[arg(long, default_value = "player")]
    name: String,
    /// Open an SDL3 window and present every frame (headless otherwise).
    #[arg(long)]
    window: bool,
    /// Window or capture size, `WIDTHxHEIGHT`.
    #[arg(long, default_value = "1280x720")]
    size: String,
    /// Persistent pipeline-cache directory.
    #[arg(long, default_value_os_t = default_gpu_cache())]
    gpu_cache: PathBuf,
    /// Override project quality: auto, low, medium, high or epic.
    #[arg(long)]
    quality: Option<QualityChoice>,
    /// Experimental hardware ray tracing; omit to use the project setting.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    ray_tracing: Option<bool>,
    /// Internal pool warm-up; only services with a Surfaces attachment request it.
    #[arg(long, hide = true)]
    prewarm_graphics: bool,
    #[command(flatten)]
    standalone: standalone::Options,
}

fn main() {
    let launched = Instant::now();
    match ara_crash::monitor_from_args() {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("ara-player crash monitor: {error:#}");
            std::process::exit(2);
        }
    }
    crash::install();
    #[cfg(feature = "tracy")]
    let _tracy = ara_gpu::__tracy::Client::start();
    let directory = std::env::var_os("ARA_CRASH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // A macOS app runs from `/` with signed contents: use the user's folder.
            cfg!(target_os = "macos")
                .then(|| ara_crash::desktop::directory().ok())
                .flatten()
                .unwrap_or_else(|| PathBuf::from(".ara/crashes"))
        });
    let build_id = std::env::var("ARA_BUILD_ID")
        .ok()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| std::fs::read(path).ok())
                .map(|bytes| format!("ara-player-{}", ara_sim::StateHash::of_bytes(&bytes)))
        })
        .unwrap_or_else(|| concat!("ara-player-", env!("CARGO_PKG_VERSION")).into());
    let _capture = match ara_crash::Capture::start(&directory, &build_id) {
        Ok(capture) => Some(capture),
        Err(error) => {
            eprintln!("ARA-EXP-CRASH-004: {error:#}");
            None
        }
    };
    if let Err(error) = run(Args::parse(), launched) {
        eprintln!("ara-player: {error:#}");
        std::process::exit(2);
    }
}

fn parse_size(text: &str) -> Result<[u32; 2]> {
    let (w, h) = text.split_once('x').context("size must be WIDTHxHEIGHT")?;
    Ok([w.parse()?, h.parse()?])
}

/// Standalone launches cache driver pipelines outside the current working folder.
/// Pools still pass their explicit cache; tests inherit their isolated config root.
fn default_gpu_cache() -> PathBuf {
    if let Some(root) = std::env::var_os("ARA_CONFIG_DIR") {
        return PathBuf::from(root).join("gpu");
    }
    for variable in ["LOCALAPPDATA", "XDG_CACHE_HOME"] {
        if let Some(root) = std::env::var_os(variable) {
            return PathBuf::from(root).join("Ara/gpu");
        }
    }
    std::env::temp_dir().join("ara-player-gpu")
}

/// Frames the live audio device keeps queued (43 ms at 48 kHz): a hardware period plus
/// a DSP quantum with headroom for a slow frame.
const AUDIO_WATERMARK: usize = 2048;

/// JSON-RPC error codes used by the player.
const ENGINE_ERROR: i64 = -32000;
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;

struct Player {
    quality: QualityChoice,
    render_options: RenderOptions,
    link: Link,
    args: Args,
    size: [u32; 2],
    window: Option<device::Windowed>,
    /// Embedded players: the shared ring lives on the presenter's device, so this
    /// field is declared (and dropped) before `presenter`.
    embed: Option<embed::Embedded>,
    presenter: Option<Presenter>,
    presenter2d: Option<Presenter2d>,
    window2d: Option<Presenter2d>,
    /// Live audio of windowed players: the world's presenter and the SDL device.
    audio: Option<(world::Audio, ara_audio::device::SdlBackend)>,
    frame: RenderFrame,
    world: Option<World>,
    pending_imports: Option<ara_play::ImportState>,
    /// Keeps the snapshot section mapped for the session (diff baselines, restarts).
    _section: Option<Section>,
    last_status: Instant,
    last_advance: Instant,
    max_late: Duration,
    /// Whether the world's first windowed frame (the full scene upload) was presented.
    uploaded: bool,
    stop: bool,
    /// Scene-dependent landscape/VFX pipelines warm on the first window frame.
    window_ready: bool,
    prewarmed: Option<(
        ara_runtime_script::ScriptPlugin,
        std::sync::mpsc::Receiver<ara_runtime_script::ScriptEvent>,
    )>,
}

fn run(args: Args, launched: Instant) -> Result<()> {
    ara_gpu::cpu_scope!("Player.run");
    if args.owner.is_none() {
        if args.game.game || game_app::has_menu(&args)? {
            return game_app::run(args);
        }
        return standalone::run(args, launched);
    }
    let init = launched;
    let size = parse_size(&args.size)?;
    let token = std::env::var(TOKEN_ENV).context("ARA_PLAY_TOKEN is not set")?;
    let prewarmed = match std::env::var("ARA_PLAY_SCRIPTS") {
        Ok(text) => {
            let scripts: ara_play::ScriptScene = serde_json::from_str(&text)?;
            let config = match scripts.host {
                ara_play::ScriptHost::Development => {
                    ara_runtime_script::ScriptHostConfig::Development(
                        ara_runtime_script::ScriptSettings {
                            host_dir: scripts.host_dir.into(),
                            compile_server: scripts.compile_server.map(Into::into),
                            modules: scripts.modules.into_iter().map(Into::into).collect(),
                        },
                    )
                }
                ara_play::ScriptHost::NativeAot { manifest } => {
                    ara_runtime_script::ScriptHostConfig::NativeAot {
                        manifest: manifest.into(),
                    }
                }
            };
            Some(ara_runtime_script::ScriptPlugin::prewarm(config).context(
                "ARA0922: C# pre-warm failed; run `ara doctor` to check the .NET 10 installation",
            )?)
        }
        Err(_) => None,
    };
    // Windowed pool players create the window, device and pipelines before Play.
    let (window, presenter) = if args.window {
        let open = if args.standalone.offscreen {
            device::Windowed::open_offscreen
        } else {
            device::Windowed::open
        };
        let (window, presenter) = open(&args.name, size, &args.gpu_cache, args.quality.unwrap_or_default(), RenderOptions { ray_tracing: args.ray_tracing.unwrap_or(false) })
            .context("ARA_GPU_BACKEND: cannot open the player renderer; run `ara doctor` and check the Vulkan loader, Vulkan 1.3 GPU/driver, and validation layers")?;
        (Some(window), Some(presenter))
    } else if args.prewarm_graphics {
        let presenter = embed::prewarm(
            &args.gpu_cache,
            RenderOptions {
                ray_tracing: args.ray_tracing.unwrap_or(false),
            },
        )?;
        (None, Some(presenter))
    } else {
        (None, None)
    };
    let mut link = SocketLink::connect(args.owner.context("Play requires an owner port")?)?;
    let hello = Hello {
        token,
        protocol: PROTOCOL.into(),
        pid: std::process::id(),
        name: args.name.clone(),
        window: args.window,
        init_ms: init.elapsed().as_secs_f64() * 1000.0,
    };
    match link.call(HELLO, &hello, Duration::from_secs(10), |_| {})? {
        Ok(_) => {}
        Err(e) => return Err(anyhow!("owner refused hello: {e}")),
    }
    let link = Link::new(link)?;
    let mut player = Player {
        quality: args.quality.unwrap_or_default(),
        render_options: RenderOptions {
            ray_tracing: args.ray_tracing.unwrap_or(false),
        },
        link,
        args,
        size,
        window,
        embed: None,
        presenter,
        presenter2d: None,
        window2d: None,
        audio: None,
        frame: RenderFrame::default(),
        world: None,
        pending_imports: None,
        _section: None,
        last_status: Instant::now(),
        last_advance: Instant::now(),
        max_late: Duration::ZERO,
        uploaded: false,
        stop: false,
        window_ready: false,
        prewarmed,
    };
    player.run()
}

impl Player {
    fn run(&mut self) -> Result<()> {
        while !self.stop {
            // Idle pool state: block on the link without spinning.
            let idle = self.world.is_none();
            let message = if idle {
                self.link.wait(Duration::from_millis(250))?
            } else {
                self.link.poll()?
            };
            if let Some(message) = message {
                self.handle(message)?;
                continue;
            }
            if idle {
                if self.last_status.elapsed() >= Duration::from_millis(250) {
                    self.status()?;
                }
            } else {
                self.frame()?;
            }
        }
        if let (Some(window), Some(presenter)) = (&mut self.window, &self.presenter) {
            window.shutdown(presenter)?;
        }
        if let (Some(window), Some(presenter)) = (&mut self.window, &self.window2d) {
            window.shutdown_2d(presenter)?;
        }
        // The shared ring goes before its device.
        self.embed = None;
        self.link.flush()?;
        Ok(())
    }

    /// One loop iteration with a world: tick by host time, ack patches, heartbeat,
    /// present (windowed), publish a shared frame (embedded, when due and a slot is
    /// free) or sleep until the next tick is due (headless).
    fn frame(&mut self) -> Result<()> {
        ara_gpu::cpu_scope!("Player.frame");
        let warming_window = self.window.is_some() && !self.window_ready;
        if warming_window {
            busy(&mut self.link, "first window frame")?;
        }
        let now = Instant::now();
        let elapsed = now - self.last_advance;
        self.last_advance = now;
        if let (Some(embed), Some(presenter)) = (&mut self.embed, &mut self.presenter)
            && let Some(desc) = embed.apply_pending(presenter)?
        {
            self.link.notify(embed::method::SURFACE, &desc)?;
            self.link.flush()?;
        }
        if let Some(world) = &mut self.world {
            // An embedded Play entry holds a local simulation (input and ticks) until
            // its first lit frame publishes (`embed` module docs).
            let holding = !world.runtime.networked()
                && self.embed.as_ref().is_some_and(embed::Embedded::entering);
            let inputs = match (&mut self.window, &mut self.embed) {
                (Some(window), _) => {
                    let (quit, inputs) = window.events(world.runtime.player())?;
                    self.stop |= quit;
                    inputs
                }
                (None, Some(embed)) if !holding => embed.input.injections(world.runtime.player()),
                (None, _) => Vec::new(),
            };
            if !inputs.is_empty()
                && let Err(e) = world.runtime.input(0, inputs)
            {
                eprintln!("ara-player: input: {e:#}");
            }
            let period = Duration::from_secs_f64(1.0 / f64::from(world.runtime.hz()));
            let advanced = if holding {
                Ok(None)
            } else {
                world.runtime.advance(elapsed)
            };
            match advanced {
                Ok(Some(frame)) if frame.ticks > 0 => {
                    self.max_late = self
                        .max_late
                        .max(period.mul_f64(f64::from(frame.alpha.clamp(0.0, 1.0) as f32)));
                }
                Ok(_) => {}
                Err(e) if world.fault.is_none() => {
                    eprintln!("ara-player: {e:#}");
                    world.fault = Some(format!("{e:#}"));
                }
                Err(_) => {}
            }
            world.mark();
            if let Some(window) = &mut self.window {
                window.update_game_ui(
                    world.ui.as_ref(),
                    &world.runtime,
                    world.runtime.player().0,
                )?;
            }
            if let Some(embed) = &mut self.embed {
                if let Some(source) = &world.ui {
                    let size = [embed.desc().width, embed.desc().height];
                    let scale = embed.scale();
                    if embed.game_ui.is_none() {
                        embed.game_ui =
                            Some(game_ui::Surface::new(source, &world.runtime, size, scale)?);
                    }
                    if let Some(ui) = &mut embed.game_ui {
                        ui.update(&world.runtime, world.runtime.player().0, size, scale)?;
                    }
                } else {
                    embed.game_ui = None;
                }
            }
            let tick = world.runtime.tick();
            let due: Vec<PendingPatch> = {
                let (ready, waiting) = std::mem::take(&mut world.pending)
                    .into_iter()
                    .partition(|p| p.tick <= tick);
                world.pending = waiting;
                ready
            };
            for patch in due {
                let result = serde_json::to_value(world.patched(patch.tick, &patch.sets))?;
                self.link.respond(patch.id, Ok(result))?;
            }
            // Windowed players present every iteration (vsync paces them), embedded
            // ones when a frame is due (their frame cap), headless ones never.
            let present = self.window.is_some() || self.embed.as_ref().is_some_and(|e| e.due(now));
            if !present {
                return self.after_frame(warming_window);
            }
            {
                ara_gpu::cpu_scope!("Player.extract");
                world.extract_frame(&mut self.frame);
            }
            if let (Some(window), Some(presenter)) = (&mut self.window, &mut self.window2d) {
                let tick = world.runtime.tick();
                window.prepare_2d(
                    presenter,
                    world.runtime.play_mut().session_mut().world_mut(),
                    tick,
                )?;
            }
            let inputs = match &self.audio {
                Some(_) => Some(world.audio_inputs()?),
                None => None,
            };
            // The first presented frame uploads the whole scene (texture decodes, mesh
            // buffers): seconds with real content, so the owner's watchdog is told
            // (windowed and embedded alike: owner calls such as `play.commands` arrive
            // mid-upload).
            if !self.uploaded {
                busy(&mut self.link, "upload")?;
                self.uploaded = true;
            }
            // Acoustics and voice updates read the simulation only (physics is not
            // stepping between ticks): a worker runs them while the GPU frame presents.
            let listener = world::listener(&self.frame);
            let simulation = world.runtime.world();
            let (presented, audio) = std::thread::scope(|scope| {
                let job = match (&mut self.audio, &inputs) {
                    (Some((audio, _)), Some(inputs)) => Some(
                        scope.spawn(|| world::update_audio(audio, simulation, &listener, inputs)),
                    ),
                    _ => None,
                };
                let presented = match (
                    &mut self.window,
                    &mut self.window2d,
                    &mut self.embed,
                    &mut self.presenter,
                ) {
                    (Some(window), Some(presenter), _, _) => {
                        window.present_2d(presenter).map(|()| None)
                    }
                    (Some(window), None, _, Some(presenter)) => window
                        .present(presenter, &self.frame, || {
                            world.fidelity_content(&self.frame)
                        })
                        .map(|()| None),
                    (None, _, Some(embed), Some(presenter)) => {
                        embed.render(presenter, &self.frame, || {
                            world.fidelity_content(&self.frame)
                        })
                    }
                    _ => Ok(None),
                };
                let audio = job.map(|job| {
                    ara_gpu::cpu_scope!("Player.audio.join");
                    job.join()
                        .unwrap_or_else(|_| Err(anyhow!("audio worker panicked")))
                });
                (presented, audio)
            });
            if let Some(ready) = presented? {
                self.link.notify(embed::method::FRAME, ready)?;
                self.link.flush()?;
            }
            let mut audio_failed = audio.and_then(Result::err);
            if audio_failed.is_none()
                && let Some((audio, device)) = &mut self.audio
                && let Err(error) = audio.pump(device, AUDIO_WATERMARK)
            {
                audio_failed = Some(error.into());
            }
            if let Some(error) = audio_failed {
                eprintln!("ara-player: audio off: {error:#}");
                self.audio = None;
            }
        }
        self.after_frame(warming_window)
    }

    /// The end of every loop iteration with a world: the first-window-frame notice,
    /// C# events, the heartbeat and, without a vsync-paced window, a short sleep.
    fn after_frame(&mut self, warming_window: bool) -> Result<()> {
        #[cfg(feature = "tracy")]
        if let Some(client) = ara_gpu::__tracy::Client::running() {
            client.frame_mark();
        }
        if warming_window {
            self.window_ready = true;
            self.link.notify(
                ara_play::BUSY,
                &ara_play::Busy {
                    what: "first window frame complete".into(),
                    max_ms: 0,
                },
            )?;
            self.link.flush()?;
        }
        self.events()?;
        if self.last_status.elapsed() >= Duration::from_millis(250) {
            self.status()?;
        }
        if self.window.is_none() {
            std::thread::sleep(Duration::from_micros(500));
        }
        Ok(())
    }

    /// Forwards C# host events (reloads, diagnostics, L4 restart requests).
    fn events(&mut self) -> Result<()> {
        for surface in [
            self.window
                .as_mut()
                .and_then(|window| window.game_ui.as_mut()),
            self.embed.as_mut().and_then(|embed| embed.game_ui.as_mut()),
        ]
        .into_iter()
        .flatten()
        {
            for diagnostic in surface.diagnostics() {
                self.link.notify(
                    ara_play::EVENT,
                    &ara_play::Event::Diagnostic {
                        diagnostic: serde_json::to_value(diagnostic)?,
                    },
                )?;
            }
        }
        if let Some(world) = &self.world {
            for event in world.events() {
                self.link.notify(ara_play::EVENT, &event)?;
            }
        }
        Ok(())
    }

    fn status(&mut self) -> Result<()> {
        let (fps, frame_ms_p95) = match (&mut self.window, &mut self.embed) {
            (Some(window), _) => window.take_stats(),
            (None, Some(embed)) => embed.take_stats(),
            (None, None) => (0.0, 0.0),
        };
        let status = match &mut self.world {
            Some(world) => Status {
                tick: world.runtime.tick(),
                paused: !world.runtime.running() && !world.runtime.networked(),
                max_late_us: self.max_late.as_micros() as u64,
                fps,
                frame_ms_p95,
                fault: world.runtime.fault().or_else(|| world.fault.clone()),
                audio: self.audio.is_some(),
                net: world.runtime.stats(),
                cells: world.resident_cells(),
            },
            None => Status {
                tick: 0,
                paused: true,
                max_late_us: 0,
                fps,
                frame_ms_p95,
                fault: None,
                audio: false,
                net: None,
                cells: Vec::new(),
            },
        };
        self.link.notify(STATUS, &status)?;
        self.max_late = Duration::ZERO;
        self.last_status = Instant::now();
        Ok(())
    }

    fn handle(&mut self, message: Message) -> Result<()> {
        let (id, method, params) = match message {
            Message::Request { id, method, params } => (id, method, params),
            Message::Notification { method, params } => return self.notification(&method, params),
            Message::Response { .. } => return Ok(()),
        };
        if method == method::PATCH {
            return self.patch(id, params);
        }
        let result = self.dispatch(&method, params);
        let reply = match result {
            Ok(value) => Ok(value),
            Err(Failure::Params(e)) => Err(error(INVALID_PARAMS, e, Value::Null)),
            Err(Failure::Unknown) => Err(error(
                METHOD_NOT_FOUND,
                format!("unknown method {method}"),
                Value::Null,
            )),
            Err(Failure::Engine(e)) => Err(error(
                ENGINE_ERROR,
                format!("{e:#}"),
                json!({"method": method}),
            )),
        };
        self.link.respond(id, reply)?;
        self.link.flush()?;
        Ok(())
    }

    fn patch(&mut self, id: Value, params: Value) -> Result<()> {
        let outcome = (|| -> std::result::Result<Option<Value>, Failure> {
            let patch: ara_play::Patch = parse(params)?;
            let world = self.world.as_mut().ok_or_else(no_world)?;
            world.runtime.local_only("play.patch")?;
            let tick = world.patch(&patch)?;
            if world.runtime.running() {
                world.pending.push(PendingPatch {
                    id: id.clone(),
                    tick,
                    sets: patch.sets,
                });
                Ok(None)
            } else {
                // A paused world reaches the boundary by stepping exactly one tick.
                world.runtime.step(1, &[])?;
                world.mark();
                Ok(Some(serde_json::to_value(
                    world.patched(tick, &patch.sets),
                )?))
            }
        })();
        match outcome {
            Ok(Some(value)) => self.link.respond(id, Ok(value))?,
            Ok(None) => {}
            Err(Failure::Params(e)) => self
                .link
                .respond(id, Err(error(INVALID_PARAMS, e, Value::Null)))?,
            Err(Failure::Engine(e)) => self
                .link
                .respond(id, Err(error(ENGINE_ERROR, format!("{e:#}"), Value::Null)))?,
            Err(Failure::Unknown) => {}
        }
        self.link.flush()?;
        Ok(())
    }

    fn dispatch(&mut self, method: &str, params: Value) -> std::result::Result<Value, Failure> {
        match method {
            method::START => {
                let start: Start = parse(params)?;
                Ok(serde_json::to_value(self.start(start)?)?)
            }
            method::STOP => {
                self.stop = true;
                Ok(json!({"tick": self.world.as_ref().map_or(0, |w| w.runtime.tick())}))
            }
            method::CRASH => {
                let crash: ara_play::Crash = parse(params)?;
                if crash.kind == "hang" {
                    self.link.suspend_heartbeat();
                }
                crash::trigger(&crash.kind)
            }
            method::STALL => {
                let stall: ara_play::Stall = parse(params)?;
                if stall.millis > 30_000 {
                    return Err(Failure::Params("stall is limited to 30 seconds".into()));
                }
                std::thread::sleep(Duration::from_millis(stall.millis));
                Ok(json!({"stalledMs":stall.millis}))
            }
            embed::method::EMBED => {
                let params: embed::EmbedParams = parse(params)?;
                Ok(serde_json::to_value(self.open_surface(&params)?)?)
            }
            embed::method::RESIZE => {
                let params: embed::ResizeParams = parse(params)?;
                let (Some(embed), Some(presenter)) = (&mut self.embed, &mut self.presenter) else {
                    return Err(not_embedded());
                };
                embed.set_scale(params.scale)?;
                let desc = embed.resize(presenter, [params.width, params.height])?;
                Ok(serde_json::to_value(desc)?)
            }
            embed::method::DEVICE => {
                let params: embed::DeviceParams = parse(params)?;
                let embed = self.embed.as_mut().ok_or_else(not_embedded)?;
                for event in &params.events {
                    embed.input.apply(*event);
                }
                Ok(json!({"accepted": params.events.len()}))
            }
            _ => self.world_method(method, params),
        }
    }

    /// Owner notifications: slot releases, raw input and asynchronous resizes of
    /// embedded players. Bad params are logged; there is no one to answer.
    fn notification(&mut self, method: &str, params: Value) -> Result<()> {
        if method == method::IMPORTS {
            let state: ara_play::ImportState = serde_json::from_value(params)?;
            if let Some(world) = &mut self.world {
                world.imports(state);
                if let Some(presenter) = &mut self.presenter {
                    presenter.update_model_placeholders(|asset| world.pending_model(asset))?;
                }
            } else {
                self.pending_imports = Some(state);
            }
            return Ok(());
        }
        // Asset publication must not make the editor wait for a player frame.
        // This is separate from D19 surface/input notifications below.
        if method == method::ASSETS {
            let result = self.world_method(method, params);
            let messages: Vec<String> = match result {
                Ok(value) => value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|row| row["error"].as_str().map(str::to_owned))
                    .collect(),
                Err(Failure::Engine(error)) => vec![format!("{error:#}")],
                Err(Failure::Params(error)) => vec![error],
                Err(Failure::Unknown) => vec!["unknown asset update method".into()],
            };
            for message in messages {
                self.link.notify(
                    ara_play::EVENT,
                    &ara_play::Event::Diagnostic {
                        diagnostic: serde_json::to_value(ara_diag::Diagnostic::warning(
                            ara_runtime::ASSET_PLACEHOLDER,
                            message,
                        ))?,
                    },
                )?;
            }
            self.link.flush()?;
            return Ok(());
        }
        let Some(embed) = &mut self.embed else {
            return Ok(());
        };
        let applied = match method {
            embed::method::RELEASE => serde_json::from_value(params)
                .map(|frame| embed.release(frame))
                .map_err(anyhow::Error::from),
            embed::method::DEVICE => serde_json::from_value::<embed::DeviceParams>(params)
                .map(|p| p.events.into_iter().for_each(|e| embed.input.apply(e)))
                .map_err(anyhow::Error::from),
            embed::method::RESIZE => serde_json::from_value::<embed::ResizeParams>(params)
                .map_err(anyhow::Error::from)
                .and_then(|p| {
                    embed.set_scale(p.scale)?;
                    embed.request_resize([p.width, p.height])
                }),
            _ => Ok(()),
        };
        if let Err(error) = applied {
            eprintln!("ara-player: {method}: {error:#}");
        }
        Ok(())
    }

    /// `play.embed`: no window; render at the owner's size into a shared ring.
    /// Creates the GPU device (announced to the watchdog) before `play.start`.
    fn open_surface(
        &mut self,
        params: &embed::EmbedParams,
    ) -> Result<ara_gpu::shared::SurfaceDesc> {
        anyhow::ensure!(
            self.window.is_none(),
            "windowed players present to their own window and cannot embed"
        );
        anyhow::ensure!(self.embed.is_none(), "this player is already embedded");
        busy(&mut self.link, "gpu")?;
        let (embedded, presenter) = embed::Embedded::open(
            params,
            &self.args.gpu_cache,
            self.quality,
            self.render_options,
            self.presenter.take(),
        )?;
        let desc = embedded.desc();
        self.size = [params.width, params.height];
        self.presenter = Some(presenter);
        self.embed = Some(embedded);
        Ok(desc)
    }

    fn world_method(&mut self, method: &str, params: Value) -> std::result::Result<Value, Failure> {
        let world = self.world.as_mut().ok_or_else(no_world)?;
        let link = &mut self.link;
        let mut last = Instant::now();
        // Explicit steps run as fast as possible; keep the owner's watchdog fed.
        let mut alive = |tick: u64| {
            if last.elapsed() >= Duration::from_millis(250) {
                last = Instant::now();
                let status = Status {
                    tick,
                    paused: true,
                    max_late_us: 0,
                    fps: 0.0,
                    frame_ms_p95: 0.0,
                    fault: None,
                    audio: false,
                    net: None,
                    cells: Vec::new(),
                };
                let _ = link.notify(STATUS, &status);
                let _ = link.flush();
            }
        };
        if matches!(
            method,
            method::PAUSE
                | method::RESUME
                | method::TIMESCALE
                | method::STEP
                | method::UNTIL
                | method::RECORD
                | method::SAVE_REPLAY
                | method::REPLAY
        ) {
            world.runtime.local_only(method)?;
        }
        let value = match method {
            method::JOIN => {
                let join: ara_play::Join = parse(params)?;
                serde_json::to_value(world.runtime.join(join.index)?)?
            }
            method::PAUSE => {
                world.runtime.pause()?;
                json!({"tick": world.runtime.tick()})
            }
            method::RESUME => {
                world.runtime.resume()?;
                self.last_advance = Instant::now();
                json!({"tick": world.runtime.tick()})
            }
            method::TIMESCALE => {
                let value = params["value"]
                    .as_f64()
                    .ok_or_else(|| Failure::Params("value must be a number".into()))?;
                world.runtime.set_timescale(value)?;
                json!({"timescale": value})
            }
            method::STEP => {
                let step: ara_play::Step = parse(params)?;
                serde_json::to_value(world.step(step.ticks, &step.inputs, &mut alive)?)?
            }
            method::UNTIL => {
                let until: ara_play::Until = parse(params)?;
                serde_json::to_value(world.until(&until, &mut alive)?)?
            }
            method::INPUT => {
                let inputs: ara_play::Inputs = parse(params)?;
                json!({"tick": world.input(&inputs.inputs)?})
            }
            method::INSPECT => {
                let inspect: ara_play::Inspect = parse(params)?;
                world.inspect(&inspect.entity, &inspect.components)?
            }
            method::READ => {
                let read: ara_play::Read = parse(params)?;
                let mut values = serde_json::Map::new();
                for path in read.paths {
                    let value = world::read_path(world.runtime.play().session(), &path)?;
                    values.insert(path, value.unwrap_or(Value::Null));
                }
                json!({"tick": world.runtime.tick(), "values": values})
            }
            method::WATCH => {
                let watch: ara_play::WatchParams = parse(params)?;
                world.watch(&watch.paths, watch.every)?;
                json!({"watching": watch.paths.len()})
            }
            method::CAPTURE => {
                let params: ara_play::CaptureParams = parse(params)?;
                if params.path.is_some() {
                    busy(link, "gpu")?;
                }
                if params.path.is_some() && self.presenter.is_none() {
                    // Headless players create a GPU device on their first image.
                    let mut presenter = Presenter::new(self.size, Vec::new(), &self.args.gpu_cache);
                    presenter.set_quality(self.quality);
                    presenter.set_render_options(self.render_options);
                    self.presenter = Some(presenter);
                }
                let presenter2d = self
                    .presenter2d
                    .get_or_insert_with(|| Presenter2d::new(&self.args.gpu_cache));
                serde_json::to_value(world.capture(
                    &params,
                    self.presenter.as_mut(),
                    Some(presenter2d),
                    &mut self.frame,
                )?)?
            }
            method::RECORD => {
                world.runtime.play_mut().record()?;
                json!({"tick": world.runtime.tick()})
            }
            method::SAVE_REPLAY => {
                let save: ara_play::SaveReplay = parse(params)?;
                world.save_replay(&save.path)?
            }
            method::REPLAY => {
                let save: ara_play::SaveReplay = parse(params)?;
                world.replay(&save.path)?
            }
            method::DIFF => {
                json!({"tick": world.runtime.tick(), "changes": world.runtime.changes()?})
            }
            method::ASSETS => {
                let pins: std::collections::BTreeMap<String, String> =
                    parse(params["assets"].clone())?;
                world.assets(&pins)?
            }
            method::COMMAND => {
                let command: ara_play::Command = parse(params)?;
                world.command(&command)?
            }
            method::COMMANDS => world.commands(),
            method::COMPILE => {
                let compile: ara_play::Compile = parse(params)?;
                world.compile(&compile)
            }
            method::SNAPSHOT => world.snapshot()?,
            method::CAPTURE_AUDIO => {
                let params: ara_play::CaptureAudio = parse(params)?;
                world.capture_audio(&params, &mut self.frame)?
            }
            _ => return Err(Failure::Unknown),
        };
        Ok(value)
    }

    fn start(&mut self, start: Start) -> Result<ara_play::Started> {
        anyhow::ensure!(
            self.world.is_none(),
            "players are single-use; this one already plays"
        );
        busy(&mut self.link, "build")?;
        let decode = Instant::now();
        let scene = match &start.snapshot {
            ara_play::SnapshotRef::Shm { section, len } => {
                let mapped = Section::open(section, *len as usize)?;
                let scene = ara_play::PlayScene::decode(mapped.bytes()).map_err(|e| anyhow!(e))?;
                self._section = Some(mapped);
                scene
            }
            ara_play::SnapshotRef::Inline { bytes } => {
                ara_play::PlayScene::decode(bytes).map_err(|e| anyhow!(e))?
            }
        };
        let decode_ms = decode.elapsed().as_secs_f64() * 1000.0;
        self.quality = self
            .args
            .quality
            .unwrap_or_else(|| scene.rendering.quality.into());
        self.render_options = RenderOptions {
            ray_tracing: self.args.ray_tracing.unwrap_or(scene.rendering.ray_tracing),
        };
        if let Some(presenter) = &mut self.presenter {
            presenter.set_quality(self.quality);
            presenter.set_render_options(self.render_options);
        }
        // Windowed and embedded players present live frames (and stream live).
        let presenting = self.window.is_some() || self.embed.is_some();
        let (mut world, build_ms) = World::build(
            &scene,
            start.role,
            start.net.as_ref(),
            presenting,
            self.prewarmed.take(),
        )?;
        world.imports(self.pending_imports.take().unwrap_or_default());
        let mut handshake = None;
        if start.role == ara_play::Role::Client {
            let t = Instant::now();
            world.runtime.connect(Duration::from_secs(20))?;
            handshake = Some(t.elapsed().as_secs_f64() * 1000.0);
        }
        if let Some(resume) = &start.resume {
            world.resume(resume)?;
        }
        if self.window.is_some() {
            let source = world.asset_source()?;
            let tick = world.runtime.tick();
            let simulation = world.runtime.play_mut().session_mut().world_mut();
            if ara_runtime_render::two_d::camera(simulation).is_some() {
                let mut presenter = Presenter2d::new(&self.args.gpu_cache);
                presenter.set_atlases(ara_runtime_render::two_d::atlases(
                    simulation,
                    source.as_ref(),
                )?);
                if let Some(window) = &mut self.window {
                    window.prepare_2d(&mut presenter, simulation, tick)?;
                }
                self.window2d = Some(presenter);
            }
        }
        if presenting {
            // Live sound for windowed and embedded play; a missing device leaves play
            // silent.
            let device = ara_audio::device::SdlBackend::open(48_000)
                .map_err(anyhow::Error::from)
                .and_then(|device| Ok((world.audio_presenter()?, device)));
            match device {
                Ok(audio) => self.audio = Some(audio),
                Err(error) => eprintln!("ara-player: no audio: {error:#}"),
            }
        }
        let mut started = world.start(start.start_paused, decode_ms + build_ms)?;
        for diagnostic in world.runtime.asset_diagnostics() {
            self.link.notify(
                ara_play::EVENT,
                &ara_play::Event::Diagnostic {
                    diagnostic: serde_json::to_value(diagnostic)?,
                },
            )?;
        }
        started.net_handshake_ms = handshake;
        if let Some(embed) = &mut self.embed {
            embed.begin_entry();
        }
        self.last_advance = Instant::now();
        self.world = Some(world);
        Ok(started)
    }
}

/// Announces known-long silent work (device creation, world build) to the owner's
/// watchdog; a hang past the deadline is still reported.
fn busy(link: &mut Link, what: &str) -> Result<()> {
    link.notify(
        ara_play::BUSY,
        &ara_play::Busy {
            what: what.into(),
            max_ms: 60_000,
        },
    )?;
    link.flush()?;
    Ok(())
}

/// Method failure classes mapped to JSON-RPC codes.
enum Failure {
    Params(String),
    Unknown,
    Engine(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Failure {
    fn from(e: E) -> Self {
        Self::Engine(e.into())
    }
}

fn no_world() -> Failure {
    Failure::Engine(anyhow!("no world: send play.start first"))
}

fn not_embedded() -> Failure {
    Failure::Engine(anyhow!(
        "this player is not embedded: send play.embed first"
    ))
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> std::result::Result<T, Failure> {
    serde_json::from_value(params).map_err(|e| Failure::Params(e.to_string()))
}

#[cfg(test)]
mod cli_tests {
    #[test]
    fn game_and_standalone_arguments_form_one_valid_command() {
        use clap::CommandFactory;
        super::Args::command().debug_assert();
    }
}
