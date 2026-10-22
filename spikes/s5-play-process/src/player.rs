//! The player process. It connects back to the host, says hello, idles until `play.start`
//! (a pre-spawned pool player waits here), then runs the 60 Hz fixed-tick loop. Between ticks it
//! serves the live link: reads are answered immediately, patches are applied at the next tick
//! boundary, and a heartbeat goes out every 250 ms.

use std::collections::BTreeSet;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bevy_ecs::world::World;
use serde_json::Value;
use tungstenite::Bytes;

use crate::crash;
use crate::model::{IdIndex, SceneId, Sim, TICK_HZ, entity_count, state_hash};
use crate::protocol::*;
use crate::registry::{Op, apply_ops, entity_json, entity_of, read_path};
use crate::rpc::{Incoming, Link, Method, PROTOCOL, RpcError};
use crate::scene::SceneDoc;
use crate::shm::SharedSection;
use crate::snapshot;

const TICK: Duration = Duration::from_nanos(1_000_000_000 / TICK_HZ as u64);
const STATUS_EVERY: Duration = Duration::from_millis(250);
/// Longest the loop sleeps, so link reads are served within about a millisecond.
const POLL: Duration = Duration::from_millis(1);
/// Bounded catch-up: after falling this many ticks behind, drop the backlog.
const MAX_BACKLOG: u32 = 5;

/// Command-line options of `ara-s5-play player`.
pub struct PlayerArgs {
    pub connect: String,
    pub token: String,
    pub name: String,
    pub role: Role,
    pub gpu_init: bool,
}

pub fn run(args: PlayerArgs) -> Result<()> {
    let started = Instant::now();
    crash::install();
    let (_gpu, gpu) = match args.gpu_init {
        true => crate::gpu::Gpu::init().map(|(g, t)| (Some(g), Some(t)))?,
        false => (None, None),
    };
    let server_socket = match args.role {
        Role::Server => Some(UdpSocket::bind("127.0.0.1:0")?),
        _ => None,
    };
    let net_addr = server_socket
        .as_ref()
        .map(|s| s.local_addr().map(|a| a.to_string()))
        .transpose()?;

    let mut link = Link::connect(&args.connect)?;
    let hello = HelloParams {
        token: args.token,
        protocol: PROTOCOL.to_owned(),
        pid: std::process::id(),
        name: args.name.clone(),
        role: args.role,
        net_addr,
        init_us: started.elapsed().as_micros() as u64,
        gpu,
    };
    let id = link.request::<Hello>(&hello)?;
    wait_response(&mut link, id).context("hello rejected")?;

    // Idle until the host presses Play. The socket is blocking, so this costs no CPU.
    loop {
        match link.poll()? {
            Some(Incoming::Request { id, method, params }) if method == Start::NAME => {
                let params = Incoming::params::<Start>(params)?;
                let session = Session::start(&mut link, id, params, &args.name, server_socket)?;
                return session.run(&mut link);
            }
            Some(Incoming::Request { id, method, .. }) if method == Stop::NAME => {
                return link.respond::<Stop>(id, Ok(TickResult { tick: 0 }));
            }
            Some(Incoming::Request { id, method, .. }) => {
                link.respond::<Stop>(id, Err(not_playing(&method)))?;
            }
            Some(_) | None => {}
        }
    }
}

fn not_playing(method: &str) -> RpcError {
    RpcError {
        code: -32000,
        message: format!("{method}: the player is idle; send play.start first"),
        data: None,
    }
}

fn wait_response(link: &mut Link, id: u64) -> Result<Value> {
    loop {
        if let Some(Incoming::Response { id: rid, result }) = link.poll()?
            && rid == id
        {
            return result.map_err(|e| anyhow!("{}", e.message));
        }
    }
}

/// The starting snapshot, kept so `play.diff` can compare against it. Decoded on demand.
enum Baseline {
    Json(Bytes),
    Binary(Bytes),
    Shm(SharedSection, usize),
}

impl Baseline {
    fn decode(&self) -> Result<World> {
        match self {
            Baseline::Json(bytes) => serde_json::from_slice::<SceneDoc>(bytes)?.into_world(),
            Baseline::Binary(bytes) => snapshot::decode(bytes),
            Baseline::Shm(section, len) => snapshot::decode(&section.bytes()[..*len]),
        }
    }
}

fn load(link: &mut Link, snapshot: &SnapshotRef) -> Result<Baseline> {
    Ok(match snapshot {
        SnapshotRef::WsJson { blob } => {
            Baseline::Json(link.take_blob(*blob).context("missing blob")?)
        }
        SnapshotRef::WsBinary { blob } => {
            Baseline::Binary(link.take_blob(*blob).context("missing blob")?)
        }
        SnapshotRef::Shm { section, len } => {
            Baseline::Shm(SharedSection::open(section, *len as usize)?, *len as usize)
        }
    })
}

/// Game-transport side of a multiplayer session (UDP hello/welcome only; no netcode).
enum Net {
    None,
    Server {
        socket: UdpSocket,
        clients: BTreeSet<SocketAddr>,
    },
    Client {
        _socket: UdpSocket,
    },
}

/// Client: say hello to the server until it welcomes us.
fn client_handshake(server: &str, name: &str) -> Result<(UdpSocket, Duration)> {
    let t = Instant::now();
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.connect(server)?;
    socket.set_read_timeout(Some(Duration::from_millis(5)))?;
    let hello = format!("ARA-HELLO {name}");
    let mut buf = [0u8; 256];
    while t.elapsed() < Duration::from_secs(5) {
        socket.send(hello.as_bytes())?;
        // Windows reports an ICMP "port unreachable" from an earlier send as a recv error
        // (WSAECONNRESET); treat every error as "try again".
        if let Ok(n) = socket.recv(&mut buf)
            && buf[..n].starts_with(b"ARA-WELCOME")
        {
            return Ok((socket, t.elapsed()));
        }
    }
    bail!("server {server} did not welcome {name}")
}

struct Session {
    sim: Sim,
    baseline: Baseline,
    paused: bool,
    /// Patch requests waiting for the next tick boundary.
    pending: Vec<(u64, Vec<Op>)>,
    net: Net,
    next_tick: Instant,
    last_status: Instant,
    max_late: Duration,
}

enum Flow {
    Continue,
    Stop,
}

impl Session {
    /// Loads the snapshot, runs tick 1 (unless paused) and answers `play.start`.
    fn start(
        link: &mut Link,
        id: u64,
        params: StartParams,
        name: &str,
        server_socket: Option<UdpSocket>,
    ) -> Result<Self> {
        let t = Instant::now();
        let baseline = load(link, &params.snapshot)?;
        let world = baseline.decode()?;
        let decode = t.elapsed();

        let (net, net_handshake_us) = match (server_socket, &params.connect_to) {
            (Some(socket), _) => {
                socket.set_nonblocking(true)?;
                (
                    Net::Server {
                        socket,
                        clients: BTreeSet::new(),
                    },
                    None,
                )
            }
            (None, Some(server)) => {
                let (socket, took) = client_handshake(server, name)?;
                (
                    Net::Client { _socket: socket },
                    Some(took.as_micros() as u64),
                )
            }
            (None, None) => (Net::None, None),
        };

        let mut session = Session {
            sim: Sim::new(world),
            baseline,
            paused: params.start_paused,
            pending: Vec::new(),
            net,
            next_tick: Instant::now(),
            last_status: Instant::now(),
            max_late: Duration::ZERO,
        };
        let t = Instant::now();
        if !session.paused {
            session.sim.tick();
            session.next_tick += TICK;
        }
        let first_tick = t.elapsed();
        let result = StartResult {
            tick: session.sim.tick_count(),
            entities: entity_count(&mut session.sim.world) as u64,
            decode_us: decode.as_micros() as u64,
            first_tick_us: first_tick.as_micros() as u64,
            net_handshake_us,
        };
        link.respond::<Start>(id, Ok(result))?;
        Ok(session)
    }

    fn run(mut self, link: &mut Link) -> Result<()> {
        link.set_nonblocking(true)?;
        loop {
            while let Some(msg) = link.poll()? {
                if let Flow::Stop = self.handle(link, msg)? {
                    return Ok(());
                }
            }
            let now = Instant::now();
            if !self.paused && now >= self.next_tick {
                self.max_late = self.max_late.max(now - self.next_tick);
                self.tick_once(link)?;
                self.next_tick += TICK;
                if now > self.next_tick + TICK * MAX_BACKLOG {
                    self.next_tick = now;
                }
            }
            self.serve_net();
            if now - self.last_status >= STATUS_EVERY {
                self.send_status(link)?;
            }
            let wake = if self.paused {
                now + POLL
            } else {
                self.next_tick.min(now + POLL)
            };
            std::thread::sleep(wake.saturating_duration_since(Instant::now()));
        }
    }

    fn send_status(&mut self, link: &mut Link) -> Result<()> {
        let clients = match &self.net {
            Net::Server { clients, .. } => clients.len() as u32,
            _ => 0,
        };
        link.notify::<Status>(&StatusParams {
            tick: self.sim.tick_count(),
            paused: self.paused,
            clients,
            max_late_us: self.max_late.as_micros() as u64,
        })?;
        self.last_status = Instant::now();
        self.max_late = Duration::ZERO;
        Ok(())
    }

    /// Server: welcome every client that says hello.
    fn serve_net(&mut self) {
        let Net::Server { socket, clients } = &mut self.net else {
            return;
        };
        let mut buf = [0u8; 256];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, from)) if buf[..n].starts_with(b"ARA-HELLO") => {
                    let name = String::from_utf8_lossy(&buf[10..n]);
                    let welcome = format!("ARA-WELCOME {name} {}", self.sim.tick_count());
                    if socket.send_to(welcome.as_bytes(), from).is_ok() {
                        clients.insert(from);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                // See `client_handshake`: stale ICMP errors surface here on Windows.
                Err(_) => {}
            }
        }
    }

    /// Applies queued patches at this tick boundary, runs one tick, then answers the patches.
    fn tick_once(&mut self, link: &mut Link) -> Result<()> {
        let applied_at = self.sim.tick_count();
        let pending = std::mem::take(&mut self.pending);
        let outcomes: Vec<_> = pending
            .into_iter()
            .map(|(id, ops)| (id, apply_ops(&mut self.sim.world, &ops), ops))
            .collect();
        self.sim.tick();
        for (id, outcome, ops) in outcomes {
            let reply = outcome
                .map(|inverse| self.patch_result(applied_at, inverse, &ops))
                .map_err(|e| RpcError::invalid(&e));
            link.respond::<Patch>(id, reply)?;
        }
        Ok(())
    }

    fn patch_result(&self, applied_at: u64, inverse: Vec<Op>, ops: &[Op]) -> PatchResult {
        let world = &self.sim.world;
        let read_back = ops
            .iter()
            .map(|op| match op {
                Op::Set { target, path, .. } => read_path(world, *target, path),
                _ => None,
            })
            .collect();
        PatchResult {
            applied_at_tick: applied_at,
            tick: self.sim.tick_count(),
            inverse,
            read_back,
        }
    }

    fn capture(&mut self) -> CaptureResult {
        CaptureResult {
            tick: self.sim.tick_count(),
            hash: hash_hex(state_hash(&mut self.sim.world)),
            entities: entity_count(&mut self.sim.world) as u64,
        }
    }

    fn handle(&mut self, link: &mut Link, msg: Incoming) -> Result<Flow> {
        let Incoming::Request { id, method, params } = msg else {
            return Ok(Flow::Continue);
        };
        match method.as_str() {
            Inspect::NAME => {
                let p = Incoming::params::<Inspect>(params)?;
                let reply = entity_of(&self.sim.world, p.target)
                    .map(|e| InspectResult {
                        tick: self.sim.tick_count(),
                        components: entity_json(&self.sim.world, e, p.components.as_deref()),
                    })
                    .map_err(|e| RpcError::invalid(&e));
                link.respond::<Inspect>(id, reply)?;
            }
            Patch::NAME => {
                let p = Incoming::params::<Patch>(params)?;
                if self.paused {
                    // Paused worlds sit at a tick boundary: apply now, without ticking.
                    let tick = self.sim.tick_count();
                    let reply = apply_ops(&mut self.sim.world, &p.ops)
                        .map(|inverse| self.patch_result(tick, inverse, &p.ops))
                        .map_err(|e| RpcError::invalid(&e));
                    link.respond::<Patch>(id, reply)?;
                } else {
                    self.pending.push((id, p.ops));
                }
            }
            Pause::NAME => {
                self.paused = true;
                link.respond::<Pause>(
                    id,
                    Ok(TickResult {
                        tick: self.sim.tick_count(),
                    }),
                )?;
            }
            Resume::NAME => {
                self.paused = false;
                self.next_tick = Instant::now();
                link.respond::<Resume>(
                    id,
                    Ok(TickResult {
                        tick: self.sim.tick_count(),
                    }),
                )?;
            }
            Step::NAME => {
                let p = Incoming::params::<Step>(params)?;
                if !self.paused {
                    let err = RpcError::invalid(&anyhow!("play.step needs a paused player"));
                    link.respond::<Step>(id, Err(err))?;
                } else {
                    for _ in 0..p.ticks {
                        self.tick_once(link)?;
                    }
                    let capture = self.capture();
                    link.respond::<Step>(id, Ok(capture))?;
                }
            }
            Capture::NAME => {
                let capture = self.capture();
                link.respond::<Capture>(id, Ok(capture))?;
            }
            Diff::NAME => {
                let p = Incoming::params::<Diff>(params)?;
                let reply = self.diff(p).map_err(|e| RpcError::invalid(&e));
                link.respond::<Diff>(id, reply)?;
            }
            Stop::NAME => {
                link.respond::<Stop>(
                    id,
                    Ok(TickResult {
                        tick: self.sim.tick_count(),
                    }),
                )?;
                return Ok(Flow::Stop);
            }
            Crash::NAME => crash::trigger(Incoming::params::<Crash>(params)?.kind),
            _ => {
                let err = RpcError {
                    code: -32601,
                    message: format!("unknown method {method}"),
                    data: None,
                };
                link.respond::<Stop>(id, Err(err))?;
            }
        }
        Ok(Flow::Continue)
    }

    /// Field-level ops that turn the starting snapshot into the current state.
    fn diff(&mut self, p: DiffParams) -> Result<DiffResult> {
        let base = self.baseline.decode()?;
        let current = &self.sim.world;
        let ids: BTreeSet<SceneId> = match p.targets {
            Some(targets) => targets.into_iter().collect(),
            None => {
                let index = |w: &World| {
                    w.resource::<IdIndex>()
                        .0
                        .keys()
                        .copied()
                        .collect::<Vec<_>>()
                };
                index(&base).into_iter().chain(index(current)).collect()
            }
        };
        let only = p.components.as_deref();
        let mut ops = Vec::new();
        for id in ids {
            match (entity_of(&base, id).ok(), entity_of(current, id).ok()) {
                (Some(b), Some(c)) => {
                    let before = entity_json(&base, b, only);
                    let after = entity_json(current, c, only);
                    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
                    for name in names {
                        match (before.get(name), after.get(name)) {
                            (Some(bv), Some(cv)) => diff_value(id, name.clone(), bv, cv, &mut ops),
                            (None, Some(cv)) => ops.push(Op::Set {
                                target: id,
                                path: name.clone(),
                                value: cv.clone(),
                                prev: None,
                            }),
                            (Some(bv), None) => ops.push(Op::Remove {
                                target: id,
                                component: name.clone(),
                                prev: Some(bv.clone()),
                            }),
                            (None, None) => {}
                        }
                    }
                }
                (None, Some(c)) => ops.push(Op::Create {
                    id: Some(id),
                    components: entity_json(current, c, None),
                }),
                (Some(b), None) => ops.push(Op::Delete {
                    target: id,
                    snapshot: Some(entity_json(&base, b, None)),
                }),
                (None, None) => {}
            }
        }
        Ok(DiffResult {
            tick: self.sim.tick_count(),
            ops,
        })
    }
}

/// Recursive object diff; anything that is not an object (numbers, arrays, strings) is a leaf.
fn diff_value(id: SceneId, path: String, before: &Value, after: &Value, ops: &mut Vec<Op>) {
    if before == after {
        return;
    }
    if let (Value::Object(b), Value::Object(a)) = (before, after) {
        let keys: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
        for key in keys {
            let null = Value::Null;
            diff_value(
                id,
                format!("{path}.{key}"),
                b.get(key).unwrap_or(&null),
                a.get(key).unwrap_or(&null),
                ops,
            );
        }
        return;
    }
    ops.push(Op::Set {
        target: id,
        path,
        value: after.clone(),
        prev: Some(before.clone()),
    });
}
