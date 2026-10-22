//! Host-side player management: spawning player processes, the localhost WebSocket listener they
//! connect back to, typed calls with hang detection, and turning a dead player into a structured
//! fault.

use std::hash::{BuildHasher, RandomState};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use bevy_ecs::world::World;
use serde::Serialize;
use serde_json::Value;

use crate::crash::CRASH_PREFIX;
use crate::protocol::*;
use crate::rpc::{Incoming, Link, Method, PROTOCOL};
use crate::scene::SceneDoc;
use crate::shm::SharedSection;
use crate::snapshot;

/// Longest a call may take before the host gives up on it.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// A playing player that sends no heartbeat for this long is declared hung.
pub const HANG_AFTER: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Snapshot transports compared by the spike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    WsJson,
    WsBinary,
    Shm,
}

impl Transport {
    pub const ALL: [Transport; 3] = [Transport::WsJson, Transport::WsBinary, Transport::Shm];

    pub fn key(self) -> &'static str {
        match self {
            Transport::WsJson => "wsJson",
            Transport::WsBinary => "wsBinary",
            Transport::Shm => "shm",
        }
    }
}

/// An encoded edit-world snapshot, ready to hand to one or more players.
pub enum Prepared {
    Json(Vec<u8>),
    Binary(Vec<u8>),
    /// `_section` keeps the mapping (and so the named section) alive while players open it.
    Shm {
        _section: SharedSection,
        name: String,
        len: usize,
    },
}

impl Prepared {
    /// Encodes the edit world for `transport`; shm sections get a fresh name from `host`.
    pub fn encode(world: &mut World, transport: Transport, host: &mut Host) -> Result<Self> {
        Ok(match transport {
            Transport::WsJson => Prepared::Json(serde_json::to_vec(&SceneDoc::from_world(world))?),
            Transport::WsBinary => Prepared::Binary(snapshot::encode(world)),
            Transport::Shm => {
                let bytes = snapshot::encode(world);
                let name = host.section_name();
                let section = SharedSection::create(&name, &bytes)?;
                Prepared::Shm {
                    _section: section,
                    name,
                    len: bytes.len(),
                }
            }
        })
    }

    pub fn len(&self) -> usize {
        match self {
            Prepared::Json(b) | Prepared::Binary(b) => b.len(),
            Prepared::Shm { len, .. } => *len,
        }
    }
}

/// How to launch one player process.
#[derive(Clone, Debug)]
pub struct Launch {
    pub role: Role,
    pub gpu_init: bool,
    /// Run this executable instead of the host's own (the cold-new-binary measurement).
    pub exe: Option<PathBuf>,
}

impl Launch {
    pub fn standalone() -> Self {
        Self {
            role: Role::Standalone,
            gpu_init: false,
            exe: None,
        }
    }
}

/// A player OS process with its captured stderr. Dropping it kills the process.
struct Proc {
    child: Child,
    stderr: Receiver<String>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// A launched process that has not connected yet.
pub struct Spawned {
    name: String,
    proc: Proc,
    spawned_at: Instant,
}

/// Owns the listener players connect back to, plus the per-session token.
pub struct Host {
    listener: TcpListener,
    url: String,
    token: String,
    exe: PathBuf,
    seq: u64,
}

fn random_hex() -> String {
    let a = RandomState::new().hash_one(std::process::id());
    let b = RandomState::new().hash_one(Instant::now());
    format!("{a:016x}{b:016x}")
}

impl Host {
    pub fn new() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("ws://{}/play", listener.local_addr()?);
        Ok(Self {
            listener,
            url,
            token: random_hex(),
            exe: std::env::current_exe()?,
            seq: 0,
        })
    }

    pub fn exe(&self) -> &PathBuf {
        &self.exe
    }

    /// A unique, unguessable shared-section name in this session's namespace.
    pub fn section_name(&mut self) -> String {
        self.seq += 1;
        format!(
            "Local\\ara-s5-{}-{}-{}",
            std::process::id(),
            self.seq,
            &self.token[..16]
        )
    }

    /// Starts a player process; it connects back on its own.
    pub fn spawn(&mut self, launch: &Launch) -> Result<Spawned> {
        self.seq += 1;
        let name = format!("player-{}", self.seq);
        let role = match launch.role {
            Role::Standalone => "standalone",
            Role::Server => "server",
            Role::Client => "client",
        };
        let exe = launch.exe.as_ref().unwrap_or(&self.exe);
        let mut cmd = Command::new(exe);
        cmd.args([
            "player",
            "--connect",
            &self.url,
            "--token",
            &self.token,
            "--name",
            &name,
        ])
        .args(["--role", role])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
        if launch.gpu_init {
            cmd.arg("--gpu-init");
        }
        let spawned_at = Instant::now();
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", exe.display()))?;
        let stderr = child.stderr.take().context("player stderr")?;
        let (tx, rx) = mpsc::channel();
        let stderr_thread = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let proc = Proc {
            child,
            stderr: rx,
            stderr_thread: Some(stderr_thread),
        };
        Ok(Spawned {
            name,
            proc,
            spawned_at,
        })
    }

    /// Accepts one connection per spawned process and completes each hello handshake.
    pub fn connect(&mut self, mut spawned: Vec<Spawned>) -> Result<Vec<Player>> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut players = Vec::with_capacity(spawned.len());
        while !spawned.is_empty() {
            if Instant::now() > deadline {
                bail!("players did not connect within {CONNECT_TIMEOUT:?}");
            }
            for s in &mut spawned {
                if let Some(status) = s.proc.child.try_wait()? {
                    let lines: Vec<String> = s.proc.stderr.try_iter().collect();
                    bail!("{} exited before connecting ({status}): {lines:?}", s.name);
                }
            }
            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_micros(100));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            // Accepted sockets inherit the listener's non-blocking mode on Windows.
            stream.set_nonblocking(false)?;
            let mut link = Link::accept(stream)?;
            link.set_read_timeout(Some(Duration::from_secs(5)))?;
            let Some(Incoming::Request { id, method, params }) = link.poll()? else {
                bail!("expected player.hello");
            };
            if method != Hello::NAME {
                bail!("expected player.hello, got {method}");
            }
            let hello = Incoming::params::<Hello>(params)?;
            if hello.token != self.token || hello.protocol != PROTOCOL {
                link.respond::<Hello>(
                    id,
                    Err(crate::rpc::RpcError {
                        code: -32001,
                        message: "bad token or protocol".into(),
                        data: None,
                    }),
                )?;
                continue;
            }
            let index = spawned
                .iter()
                .position(|s| s.name == hello.name)
                .ok_or_else(|| anyhow!("unexpected player {}", hello.name))?;
            let s = spawned.swap_remove(index);
            link.respond::<Hello>(
                id,
                Ok(HelloResult {
                    session: s.name.clone(),
                }),
            )?;
            if std::env::var_os("ARA_S5_TRACE").is_some() {
                eprintln!(
                    "[trace] {} spawn→hello {:.1} ms (player init {} us)",
                    s.name,
                    s.spawned_at.elapsed().as_secs_f64() * 1e3,
                    hello.init_us
                );
            }
            players.push(Player {
                name: s.name,
                hello,
                link,
                proc: s.proc,
                spawned_at: s.spawned_at,
                connected_at: Instant::now(),
                playing: false,
                last_status: None,
                last_heartbeat: Instant::now(),
            });
        }
        Ok(players)
    }

    /// Spawns and connects a single player.
    pub fn launch(&mut self, launch: &Launch) -> Result<Player> {
        let spawned = self.spawn(launch)?;
        Ok(self.connect(vec![spawned])?.remove(0))
    }
}

/// Why a call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The link closed: the process exited or crashed.
    LinkClosed,
    /// No heartbeat for [`HANG_AFTER`] while playing.
    Unresponsive,
}

/// A call error that remembers whether the player itself failed.
#[derive(Debug)]
pub struct CallError {
    pub failure: Option<Failure>,
    pub error: anyhow::Error,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.error)
    }
}

impl std::error::Error for CallError {}

/// A remedy the editor offers next to a fault.
#[derive(Serialize, Debug, Clone)]
pub struct FaultAction {
    pub id: &'static str,
    pub title: &'static str,
}

/// The structured error the host reports when a player dies or hangs.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayerFault {
    pub code: &'static str,
    pub player: String,
    pub message: String,
    /// The player's own `ARA_PLAYER_CRASH` report, when it managed to write one.
    pub cause: Option<Value>,
    pub exit_code: Option<String>,
    pub last_tick: Option<u64>,
    pub stderr_tail: Vec<String>,
    pub actions: Vec<FaultAction>,
}

/// A connected player process. Dropping it kills the process.
pub struct Player {
    pub name: String,
    pub hello: HelloParams,
    pub link: Link,
    proc: Proc,
    pub spawned_at: Instant,
    pub connected_at: Instant,
    playing: bool,
    pub last_status: Option<StatusParams>,
    last_heartbeat: Instant,
}

impl Player {
    pub fn pid(&self) -> u32 {
        self.proc.child.id()
    }

    /// Working set and private (pagefile-backed) bytes of the player process.
    #[cfg(windows)]
    pub fn memory(&self) -> Option<(u64, u64)> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: the child handle is live while `self` owns it, and `counters` is a writable
        // struct whose size is passed in `cb`.
        let ok = unsafe {
            GetProcessMemoryInfo(self.proc.child.as_raw_handle(), &mut counters, counters.cb)
        };
        (ok != 0).then_some((
            counters.WorkingSetSize as u64,
            counters.PagefileUsage as u64,
        ))
    }

    #[cfg(not(windows))]
    pub fn memory(&self) -> Option<(u64, u64)> {
        None
    }

    /// Calls `M` and waits for its result.
    pub fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Output, CallError> {
        let id = self.link.request::<M>(params).map_err(|error| CallError {
            failure: Some(Failure::LinkClosed),
            error,
        })?;
        self.wait::<M>(id)
    }

    /// Waits for the response to request `id`, recording heartbeats on the way.
    pub fn wait<M: Method>(&mut self, id: u64) -> Result<M::Output, CallError> {
        let deadline = Instant::now() + CALL_TIMEOUT;
        loop {
            let now = Instant::now();
            if now > deadline {
                return Err(CallError {
                    failure: None,
                    error: anyhow!("{} timed out", M::NAME),
                });
            }
            let polled = self
                .link
                .set_read_timeout(Some(Duration::from_millis(50)))
                .and_then(|()| self.link.poll());
            match polled {
                Ok(Some(Incoming::Response { id: rid, result })) if rid == id => {
                    let value = result.map_err(|e| CallError {
                        failure: None,
                        error: anyhow!("{} failed: {}", M::NAME, e.message),
                    })?;
                    return serde_json::from_value(value).map_err(|e| CallError {
                        failure: None,
                        error: anyhow!("{} result: {e}", M::NAME),
                    });
                }
                Ok(Some(Incoming::Notification { method, params })) if method == Status::NAME => {
                    self.last_heartbeat = Instant::now();
                    self.last_status = serde_json::from_value(params).ok();
                }
                // Judge staleness only once the socket is drained: heartbeats queued while the
                // host was busy elsewhere still count.
                Ok(None) if self.playing && Instant::now() - self.last_heartbeat > HANG_AFTER => {
                    let error = anyhow!("{}: no heartbeat for {HANG_AFTER:?}", self.name);
                    return Err(CallError {
                        failure: Some(Failure::Unresponsive),
                        error,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(CallError {
                        failure: Some(Failure::LinkClosed),
                        error,
                    });
                }
            }
        }
    }

    /// Waits until the player fails (used after a deliberate crash).
    pub fn wait_failure(&mut self) -> Failure {
        loop {
            // No request is outstanding, so `wait` only returns on failure or timeout.
            if let Err(e) = self.wait::<Capture>(u64::MAX)
                && let Some(failure) = e.failure
            {
                return failure;
            }
        }
    }

    /// Sends the snapshot and `play.start`; returns the request id to wait on.
    pub fn send_start(
        &mut self,
        snapshot: &Prepared,
        start_paused: bool,
        connect_to: Option<String>,
    ) -> Result<u64> {
        let snapshot = match snapshot {
            Prepared::Json(bytes) | Prepared::Binary(bytes) => {
                let (blob, mut frame) = self.link.blob_frame();
                frame.extend_from_slice(bytes);
                self.link.send_blob(frame)?;
                if matches!(snapshot, Prepared::Json(_)) {
                    SnapshotRef::WsJson { blob }
                } else {
                    SnapshotRef::WsBinary { blob }
                }
            }
            Prepared::Shm { name, len, .. } => SnapshotRef::Shm {
                section: name.clone(),
                len: *len as u64,
            },
        };
        let id = self.link.request::<Start>(&StartParams {
            snapshot,
            start_paused,
            connect_to,
        })?;
        self.playing = true;
        self.last_heartbeat = Instant::now();
        Ok(id)
    }

    /// Sends the snapshot, starts playing and waits for the first tick.
    pub fn start(&mut self, snapshot: &Prepared, start_paused: bool) -> Result<StartResult> {
        let id = self.send_start(snapshot, start_paused, None)?;
        Ok(self.wait::<Start>(id)?)
    }

    /// Asks the player to stop and waits for it to exit (killing it after 2 s).
    pub fn stop(mut self) -> Result<()> {
        let result = self.call::<Stop>(&Empty {});
        wait_exit(&mut self.proc.child, Duration::from_secs(2));
        result.map(|_| ()).map_err(|e| e.error)
    }

    /// Classifies a failed player and reaps it. Hung players are killed.
    pub fn fault(mut self, failure: Failure) -> PlayerFault {
        let status = match failure {
            Failure::LinkClosed => wait_exit(&mut self.proc.child, Duration::from_secs(2)),
            Failure::Unresponsive => None,
        };
        if status.is_none() {
            let _ = self.proc.child.kill();
        }
        let _ = self.proc.child.wait();
        if let Some(thread) = self.proc.stderr_thread.take() {
            let _ = thread.join();
        }
        let stderr: Vec<String> = self.proc.stderr.try_iter().collect();
        let cause = stderr
            .iter()
            .find_map(|l| l.strip_prefix(CRASH_PREFIX))
            .and_then(|json| serde_json::from_str::<Value>(json).ok());
        let exit = status.and_then(|s| s.code()).map(|c| c as u32);
        let (code, message) = match (failure, exit) {
            (Failure::Unresponsive, _) => (
                "player_unresponsive",
                format!("no heartbeat for {HANG_AFTER:?}; the player was killed"),
            ),
            (_, Some(101)) => ("player_panicked", "the player panicked".to_owned()),
            (_, Some(0xC000_0005)) => (
                "player_crashed",
                "the player crashed with an access violation".to_owned(),
            ),
            (_, Some(0)) => ("player_exited", "the player exited".to_owned()),
            (_, Some(code)) => (
                "player_crashed",
                format!("the player exited with 0x{code:08X}"),
            ),
            (_, None) => ("player_crashed", "the player's link closed".to_owned()),
        };
        let tail = stderr.len().saturating_sub(8);
        PlayerFault {
            code,
            player: self.name.clone(),
            message,
            cause,
            exit_code: exit.map(|c| format!("0x{c:08X}")),
            last_tick: self.last_status.as_ref().map(|s| s.tick),
            stderr_tail: stderr[tail..].to_vec(),
            actions: vec![FaultAction {
                id: "restart",
                title: "Restart play from the same snapshot",
            }],
        }
    }
}

fn wait_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}
