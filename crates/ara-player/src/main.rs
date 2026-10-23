//! `ara-player`: the S5 play process. Spawned by the edit-world owner (the `ara`
//! daemon or the editor) into a pre-spawned, single-use pool. It initializes, connects
//! back over localhost WebSocket with the session token and idles at zero CPU until
//! `play.start` assigns a role and a shared-memory snapshot; then it runs that world
//! (windowed through SDL3, or headless) and exits on `play.stop`.
mod crash;
mod device;
mod world;

use anyhow::{Context, Result, anyhow};
use ara_play::{
    HELLO, Hello, PROTOCOL, STATUS, Start, Status, TOKEN_ENV,
    link::{Link, Message, error},
    method,
    shm::Section,
};
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, two_d::Presenter2d};
use clap::Parser;
use serde_json::{Value, json};
use std::{
    net::UdpSocket,
    path::PathBuf,
    time::{Duration, Instant},
};
use world::{PendingPatch, World};

/// Command line. The session token comes from `ARA_PLAY_TOKEN`, never an argument.
#[derive(Parser)]
#[command(
    name = "ara-player",
    about = "Ara play process (spawned by the edit-world owner)"
)]
struct Args {
    /// Owner's live-link port on 127.0.0.1.
    #[arg(long)]
    owner: u16,
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
    #[arg(long, default_value = ".ara/gpu")]
    gpu_cache: PathBuf,
}

fn main() {
    crash::install();
    if let Err(error) = run(Args::parse()) {
        eprintln!("ara-player: {error:#}");
        std::process::exit(2);
    }
}

fn parse_size(text: &str) -> Result<[u32; 2]> {
    let (w, h) = text.split_once('x').context("size must be WIDTHxHEIGHT")?;
    Ok([w.parse()?, h.parse()?])
}

/// Frames the live audio device keeps queued (43 ms at 48 kHz): a hardware period plus
/// a DSP quantum with headroom for a slow frame.
const AUDIO_WATERMARK: usize = 2048;

/// JSON-RPC error codes used by the player.
const ENGINE_ERROR: i64 = -32000;
const INVALID_PARAMS: i64 = -32602;
const METHOD_NOT_FOUND: i64 = -32601;

struct Player {
    link: Link,
    args: Args,
    size: [u32; 2],
    window: Option<device::Windowed>,
    presenter: Option<Presenter>,
    presenter2d: Option<Presenter2d>,
    /// Live audio of windowed players: the world's presenter and the SDL device.
    audio: Option<(
        ara_runtime_audio::AudioPresenter,
        ara_audio::device::SdlBackend,
    )>,
    frame: RenderFrame,
    world: Option<World>,
    /// Keeps the snapshot section mapped for the session (diff baselines, restarts).
    _section: Option<Section>,
    udp: UdpSocket,
    last_status: Instant,
    last_advance: Instant,
    max_late: Duration,
    stop: bool,
}

fn run(args: Args) -> Result<()> {
    let init = Instant::now();
    let size = parse_size(&args.size)?;
    let token = std::env::var(TOKEN_ENV).context("ARA_PLAY_TOKEN is not set")?;
    // Server roles answer client handshakes on this socket (the ara-net seam).
    let udp = UdpSocket::bind("127.0.0.1:0")?;
    udp.set_nonblocking(true)?;
    // Windowed pool players create the window, device and pipelines before Play.
    let (window, presenter) = if args.window {
        let (window, presenter) = device::Windowed::open(&args.name, size, &args.gpu_cache)?;
        (Some(window), Some(presenter))
    } else {
        (None, None)
    };
    let mut link = Link::connect(args.owner)?;
    let hello = Hello {
        token,
        protocol: PROTOCOL.into(),
        pid: std::process::id(),
        name: args.name.clone(),
        window: args.window,
        init_ms: init.elapsed().as_secs_f64() * 1000.0,
        net_addr: Some(udp.local_addr()?.to_string()),
    };
    match link.call(HELLO, &hello, Duration::from_secs(10), |_| {})? {
        Ok(_) => {}
        Err(e) => return Err(anyhow!("owner refused hello: {e}")),
    }
    let mut player = Player {
        link,
        args,
        size,
        window,
        presenter,
        presenter2d: None,
        audio: None,
        frame: RenderFrame::default(),
        world: None,
        _section: None,
        udp,
        last_status: Instant::now(),
        last_advance: Instant::now(),
        max_late: Duration::ZERO,
        stop: false,
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
            self.answer_handshakes();
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
        self.link.flush()?;
        Ok(())
    }

    /// One loop iteration with a world: tick by host time, ack patches, heartbeat,
    /// present (windowed) or sleep until the next tick is due (headless).
    fn frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let elapsed = now - self.last_advance;
        self.last_advance = now;
        if let Some(world) = &mut self.world {
            let running = world.runtime.running();
            if let Some(window) = &mut self.window
                && window.events(&mut world.runtime)?
            {
                self.stop = true;
            }
            if running {
                let period = Duration::from_secs_f64(1.0 / f64::from(world.runtime.hz()));
                match world.runtime.advance(elapsed) {
                    Ok(frame) => {
                        if frame.ticks > 0 {
                            self.max_late = self
                                .max_late
                                .max(period.mul_f64(f64::from(frame.alpha.clamp(0.0, 1.0) as f32)));
                        }
                    }
                    Err(e) => eprintln!("ara-player: {e}"),
                }
                world.mark();
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
            if let (Some(window), Some(presenter)) = (&mut self.window, &mut self.presenter) {
                window.present(&mut world.runtime, presenter, &mut self.frame)?;
            }
            let mut audio_failed = None;
            if let Some((audio, device)) = &mut self.audio {
                let result = world
                    .audio(audio, &mut self.frame)
                    .and_then(|()| Ok(audio.pump(device, AUDIO_WATERMARK)?));
                audio_failed = result.err();
            }
            if let Some(error) = audio_failed {
                eprintln!("ara-player: audio off: {error:#}");
                self.audio = None;
            }
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
        if let Some(world) = &self.world {
            for event in world.events() {
                self.link.notify(ara_play::EVENT, &event)?;
            }
        }
        Ok(())
    }

    fn status(&mut self) -> Result<()> {
        let (fps, frame_ms_p95) = self
            .window
            .as_mut()
            .map_or((0.0, 0.0), device::Windowed::take_stats);
        let status = match &self.world {
            Some(world) => Status {
                tick: world.runtime.tick(),
                paused: !world.runtime.running(),
                max_late_us: self.max_late.as_micros() as u64,
                fps,
                frame_ms_p95,
                fault: world.runtime.fault(),
                audio: self.audio.is_some(),
            },
            None => Status {
                tick: 0,
                paused: true,
                max_late_us: 0,
                fps,
                frame_ms_p95,
                fault: None,
                audio: false,
            },
        };
        self.link.notify(STATUS, &status)?;
        self.max_late = Duration::ZERO;
        self.last_status = Instant::now();
        Ok(())
    }

    /// Server roles answer `hello` datagrams with `welcome`.
    fn answer_handshakes(&mut self) {
        let mut buffer = [0u8; 64];
        // WSAECONNRESET from an ICMP unreachable is "retry", like WouldBlock.
        while let Ok((n, from)) = self.udp.recv_from(&mut buffer) {
            if &buffer[..n] == b"ara-hello" {
                let _ = self.udp.send_to(b"ara-welcome", from);
            }
        }
    }

    fn handle(&mut self, message: Message) -> Result<()> {
        let Message::Request { id, method, params } = message else {
            return Ok(());
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
                crash::trigger(&crash.kind)
            }
            _ => self.world_method(method, params),
        }
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
                };
                let _ = link.notify(STATUS, &status);
                let _ = link.flush();
            }
        };
        let value = match method {
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
                    self.presenter =
                        Some(Presenter::new(self.size, Vec::new(), &self.args.gpu_cache));
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
        let mut handshake = None;
        if let (ara_play::Role::Client, Some(server)) = (start.role, &start.connect_to) {
            let t = Instant::now();
            client_handshake(&self.udp, server)?;
            handshake = Some(t.elapsed().as_secs_f64() * 1000.0);
        }
        let (mut world, build_ms) = World::build(&scene)?;
        if let Some(resume) = &start.resume {
            world.resume(resume)?;
        }
        if self.window.is_some() {
            // Live sound for windowed play; a missing device leaves play silent.
            let device = ara_audio::device::SdlBackend::open(48_000)
                .map_err(anyhow::Error::from)
                .and_then(|device| Ok((world.audio_presenter()?, device)));
            match device {
                Ok(audio) => self.audio = Some(audio),
                Err(error) => eprintln!("ara-player: no audio: {error:#}"),
            }
        }
        let mut started = world.start(start.start_paused, decode_ms + build_ms)?;
        started.net_handshake_ms = handshake;
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

/// Client roles complete a UDP hello/welcome with their server before reporting
/// running (S5). `ara-net` replaces this with its encrypted transport.
fn client_handshake(udp: &UdpSocket, server: &str) -> Result<()> {
    let start = Instant::now();
    let mut buffer = [0u8; 64];
    while start.elapsed() < Duration::from_secs(5) {
        udp.send_to(b"ara-hello", server)?;
        let wait = Instant::now();
        while wait.elapsed() < Duration::from_millis(20) {
            if let Ok((n, _)) = udp.recv_from(&mut buffer)
                && &buffer[..n] == b"ara-welcome"
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    Err(anyhow!("server {server} did not answer the handshake"))
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

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> std::result::Result<T, Failure> {
    serde_json::from_value(params).map_err(|e| Failure::Params(e.to_string()))
}
