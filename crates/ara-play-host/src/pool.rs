//! Pre-spawned, single-use `ara-player` processes (S5): spawned ahead of Play,
//! authenticated by the per-session token in `player.hello`, idle at zero CPU until
//! taken. Faults are classified from the exit code, the stderr crash line and the
//! Independent 250 ms heartbeats (five seconds of silence, beyond announced busy
//! grace, is a hang; the player is killed).
//! Launches run on background threads (CreateProcess of the player image costs
//! 10-200 ms on Windows), and players that exit before their hello back the refill
//! off (0.5 s doubling to 30 s), so a broken player never stalls the host tick.
use ara_play::{
    BUSY, Busy, CRASH_PREFIX, CrashReport, HELLO, Hello, PROTOCOL, STATUS, Status, TOKEN_ENV,
    link::{Link, Message, error},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// How long a running player may stay silent before it is declared hung.
pub const WATCHDOG: Duration = Duration::from_secs(5);

/// A connected player process.
pub struct Player {
    /// Opaque lifetime, independent of PID reuse or reconnect timing.
    pub(crate) incarnation: String,
    /// Owned child; killed on drop.
    pub child: crate::group::ChildHandle,
    /// Live link.
    pub link: Link,
    /// Its hello (pid, window, init time, UDP address).
    pub hello: Hello,
    /// Stderr log file.
    pub log: PathBuf,
    /// Last heartbeat or message.
    pub seen: Instant,
    /// Latest status heartbeat.
    pub status: Option<Status>,
    /// Silence allowed until this instant (announced long work).
    pub grace: Instant,
    /// `play.event` notifications (C# reloads, diagnostics, restarts) not yet drained.
    pub events: Vec<ara_play::Event>,
    /// Embedded players' surface and frame notices not yet drained.
    pub embed: Vec<crate::embed::Notice>,
    /// Bounded nonblocking service requests; replies survive ordinary heartbeat polling.
    pub(crate) replies: Replies,
}

#[derive(Default)]
pub(crate) struct Replies(std::collections::BTreeMap<u64, Option<Result<Value, Value>>>);
impl Replies {
    fn reserve(&mut self, id: u64) -> bool {
        if self.0.len() >= 64 {
            return false;
        }
        self.0.insert(id, None);
        true
    }
    fn observe(&mut self, message: &Message) {
        if let Message::Response { id, result } = message
            && let Some(id) = id.as_u64()
            && let Some(reply) = self.0.get_mut(&id)
        {
            *reply = Some(result.clone());
        }
    }
    fn take(&mut self, id: u64) -> Option<Result<Value, Value>> {
        let reply = self.0.get_mut(&id)?.take()?;
        self.0.remove(&id);
        Some(reply)
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;
    #[test]
    fn out_of_order_job_replies_survive_polling_and_are_taken_once() {
        let mut replies = Replies::default();
        assert!(replies.reserve(1));
        assert!(replies.reserve(2));
        replies.observe(&Message::Response {
            id: json!(2),
            result: Ok(json!({"tick":42})),
        });
        replies.observe(&Message::Response {
            id: json!(99),
            result: Ok(json!("unsolicited")),
        });
        assert!(replies.take(1).is_none());
        assert_eq!(replies.take(2), Some(Ok(json!({"tick":42}))));
        assert!(replies.take(2).is_none());
        assert_eq!(replies.0.len(), 1);
        replies.observe(&Message::Response {
            id: json!(1),
            result: Err(json!({"message":"rejected"})),
        });
        assert_eq!(replies.take(1), Some(Err(json!({"message":"rejected"}))));
        assert!(replies.0.is_empty());
    }
    #[test]
    fn mailbox_is_bounded_and_canceled_requests_cannot_reappear() {
        let mut replies = Replies::default();
        for id in 0..64 {
            assert!(replies.reserve(id));
        }
        assert!(!replies.reserve(64));
        replies.0.remove(&12);
        replies.observe(&Message::Response {
            id: json!(12),
            result: Ok(Value::Null),
        });
        assert_eq!(replies.0.len(), 63);
        assert!(replies.reserve(64));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Player {
    /// Handles a non-response message (heartbeats); anything arriving counts as alive.
    pub fn observe(&mut self, message: &Message) {
        self.seen = Instant::now();
        self.replies.observe(message);
        if let Message::Notification { method, params } = message {
            if method == STATUS {
                self.status = serde_json::from_value(params.clone()).ok();
            } else if method == BUSY
                && let Ok(busy) = serde_json::from_value::<Busy>(params.clone())
            {
                self.grace = Instant::now() + Duration::from_millis(busy.max_ms.min(300_000));
            } else if method == ara_play::EVENT
                && let Ok(event) = serde_json::from_value(params.clone())
            {
                self.events.push(event);
            } else if method == crate::embed::method::FRAME
                && let Ok(ready) = serde_json::from_value(params.clone())
            {
                self.embed.push(crate::embed::Notice::Frame {
                    ready,
                    received_at_us: ara_gpu::shared::now_us(),
                });
            } else if method == crate::embed::method::SURFACE
                && let Ok(desc) = serde_json::from_value(params.clone())
            {
                self.embed.push(crate::embed::Notice::Surface(desc));
            }
        }
    }

    /// Silent past the watchdog and past any announced grace.
    fn hung(&self) -> bool {
        self.seen.elapsed() > WATCHDOG && Instant::now() > self.grace
    }

    /// Calls a player method, keeping heartbeats flowing. A dead or silent player
    /// yields the classified fault instead of a bare timeout.
    pub fn call(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Result<Value, Value>, Value> {
        self.call_checked(method, params, timeout, || Ok(()))
    }

    /// Queue a request without waiting for its player tick or response. `check`
    /// keeps its bounded reply alive alongside heartbeats and embedded frame notices.
    pub(crate) fn request(&mut self, method: &str, params: &Value) -> Result<u64, Value> {
        if self.replies.0.len() >= 64 {
            return Err(error(
                -32000,
                "too many pending player requests",
                Value::Null,
            ));
        }
        let id = self
            .link
            .request(method, params)
            .map_err(|problem| error(-32000, problem.to_string(), Value::Null))?;
        if !self.replies.reserve(id) {
            return Err(error(
                -32000,
                "too many pending player requests",
                Value::Null,
            ));
        }
        if let Err(problem) = self.link.flush() {
            self.replies.0.remove(&id);
            return Err(error(-32000, problem.to_string(), Value::Null));
        }
        Ok(id)
    }
    pub(crate) fn response(&mut self, id: u64) -> Option<Result<Value, Value>> {
        self.replies.take(id)
    }
    pub(crate) fn forget_request(&mut self, id: u64) {
        self.replies.0.remove(&id);
    }

    /// Startup-only cancellable wait. Ordinary calls retain their synchronous contract.
    pub(crate) fn call_checked(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
        mut check: impl FnMut() -> Result<(), &'static str>,
    ) -> Result<Result<Value, Value>, Value> {
        let id = match self.link.request(method, params) {
            Ok(id) => json!(id),
            Err(_) => return Err(self.closed()),
        };
        let started = Instant::now();
        self.seen = Instant::now();
        loop {
            if let Err(message) = check() {
                return Ok(Err(error(-32000, message, Value::Null)));
            }
            match self.link.wait(Duration::from_millis(20)) {
                Ok(Some(Message::Response { id: got, result })) if got == id => {
                    // The announced long work belonged to this request.
                    self.seen = Instant::now();
                    self.grace = self.seen;
                    return Ok(result);
                }
                Ok(Some(message)) => self.observe(&message),
                Ok(None) => {}
                Err(_) => return Err(self.closed()),
            }
            // IO-thread heartbeats continue through simulation and uploads.
            if self.hung() {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(unresponsive(self));
            }
            if started.elapsed() > timeout {
                return Ok(Err(error(
                    -32000,
                    format!("player did not answer `{method}` within {timeout:?}"),
                    Value::Null,
                )));
            }
        }
    }

    /// The link broke: wait briefly for the exit status, then classify it.
    fn closed(&mut self) -> Value {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Some(fault) = self.fault() {
                return fault;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        json!({"code": "player_exited", "message": "player link closed", "pid": self.hello.pid,
               "actions": [{"id": "restart", "title": "Restart play from the same snapshot"}]})
    }

    /// Drains heartbeats; returns a fault if the player died or hung.
    pub fn check(&mut self) -> Option<Value> {
        for _ in 0..256 {
            match self.link.poll() {
                Ok(Some(message)) => self.observe(&message),
                Ok(None) => break,
                Err(_) => break,
            }
        }
        if let Some(fault) = self.fault() {
            return Some(fault);
        }
        if self.hung() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            return Some(unresponsive(self));
        }
        None
    }

    /// The classified exit, if the process ended.
    pub fn fault(&mut self) -> Option<Value> {
        let status = self.child.try_wait().ok().flatten()?;
        let tail = tail(&self.log);
        let report = tail
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix(CRASH_PREFIX))
            .and_then(|json| serde_json::from_str::<CrashReport>(json).ok());
        let code = status.code().map(|c| c as u32);
        let kind = match (&report, code) {
            (Some(r), _) if r.kind == "panic" => "player_panicked",
            (Some(_), _) => "player_crashed",
            (None, Some(0)) => "player_exited",
            (None, Some(c)) if c >= 0xC000_0000 => "player_crashed",
            _ => "player_exited",
        };
        Some(json!({
            "code": kind,
            "message": match &report {
                Some(r) => format!("{kind}: {} at {}", r.message, r.location),
                None => format!("{kind}: exit code {code:?}"),
            },
            "exitCode": code.map(|c| format!("0x{c:08X}")),
            "cause": report,
            "lastTick": self.status.as_ref().map(|s| s.tick),
            "stderrTail": tail,
            "pid": self.hello.pid,
            "actions": [{"id": "restart", "title": "Restart play from the same snapshot"}],
        }))
    }
}

fn unresponsive(player: &Player) -> Value {
    json!({
        "code": "player_unresponsive",
        "message": format!("player_unresponsive: no heartbeat for {:?}; killed", player.seen.elapsed()),
        "lastTick": player.status.as_ref().map(|s| s.tick),
        "stderrTail": tail(&player.log),
        "pid": player.hello.pid,
        "actions": [{"id": "restart", "title": "Restart play from the same snapshot"}],
    })
}

/// The last 4 KiB of a log file.
fn tail(path: &Path) -> String {
    (|| -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        file.take(4096).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })()
    .unwrap_or_default()
}

/// A spawned process that has not said hello yet.
struct Spawning {
    child: Child,
    log: PathBuf,
    window: bool,
    spawned: Instant,
}

/// A launch finished on its thread: the process, or why it could not start.
struct Launched {
    window: bool,
    result: std::io::Result<Spawning>,
}

/// Where players come from.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// The `ara-player` executable.
    pub executable: PathBuf,
    /// Directory for player stderr logs.
    pub logs: PathBuf,
    /// Pipeline-cache directory passed to players.
    pub gpu_cache: PathBuf,
    /// Idle headless players to keep ready.
    pub headless: usize,
    /// Idle windowed players to keep ready (each owns a hidden-until-Play window).
    pub windowed: usize,
    /// Window size for windowed players.
    pub size: [u32; 2],
}

/// The pool and the live-link listener.
pub struct Pool {
    config: PoolConfig,
    listener: TcpListener,
    token: String,
    spawning: Vec<Spawning>,
    idle: Vec<Player>,
    next: u32,
    scripts: Option<ara_play::ScriptScene>,
    rendering: ara_play::RenderSettings,
    graphics_prewarm: bool,
    /// Launch results, sent by each launch thread.
    launches: (mpsc::Sender<Launched>, mpsc::Receiver<Launched>),
    /// Launches in flight, `[headless, windowed]`.
    launching: [usize; 2],
    /// Consecutive players that failed to start or exited before their hello.
    failures: u32,
    /// Refills wait until then after a failure.
    retry_at: Option<Instant>,
}

impl Pool {
    #[cfg(test)]
    pub(crate) fn with_idle(
        config: PoolConfig,
        token: String,
        idle: Vec<Player>,
    ) -> std::io::Result<Self> {
        let mut pool = Self::new(config, token)?;
        pool.idle = idle;
        Ok(pool)
    }
    /// Launch independently owned headless/embedded player worlds in one process.
    /// Each has a separate authenticated link, runtime, inputs and shared GPU surface.
    pub fn take_shared(&mut self, count: usize, timeout: Duration) -> Result<Vec<Player>, String> {
        self.take_shared_checked(count, timeout, || Ok(()))
    }
    pub(crate) fn take_shared_checked(
        &mut self,
        count: usize,
        timeout: Duration,
        check: impl FnMut() -> Result<(), &'static str>,
    ) -> Result<Vec<Player>, String> {
        self.next += 1;
        let request = crate::group::Request {
            name: format!("shared{}", self.next),
            count,
            timeout,
        };
        crate::group::spawn(
            &self.config,
            &self.token,
            self.scripts.as_ref(),
            self.rendering,
            request,
            check,
        )
    }
    /// Binds the live-link listener on 127.0.0.1 and starts filling the pool.
    pub fn new(config: PoolConfig, token: String) -> std::io::Result<Self> {
        Self::with_scripts(config, token, None)
    }

    /// Warms project modules in each single-use player before its ready handshake.
    pub fn with_scripts(
        config: PoolConfig,
        token: String,
        scripts: Option<ara_play::ScriptScene>,
    ) -> std::io::Result<Self> {
        Self::with_rendering(config, token, scripts, Default::default())
    }

    /// Select the logical-device policy before windowed/embedded GPU warm-up.
    /// Changing this setting requires a newly opened pool/device.
    pub fn with_rendering(
        config: PoolConfig,
        token: String,
        scripts: Option<ara_play::ScriptScene>,
        rendering: ara_play::RenderSettings,
    ) -> std::io::Result<Self> {
        Self::with_presenter(config, token, scripts, rendering, false)
    }

    /// Surface-attached services warm a small headless presenter before hello.
    pub(crate) fn with_presenter(
        config: PoolConfig,
        token: String,
        scripts: Option<ara_play::ScriptScene>,
        rendering: ara_play::RenderSettings,
        graphics_prewarm: bool,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        std::fs::create_dir_all(&config.logs)?;
        let mut pool = Self {
            config,
            listener,
            token,
            spawning: Vec::new(),
            idle: Vec::new(),
            next: 0,
            scripts,
            rendering,
            graphics_prewarm,
            launches: mpsc::channel(),
            launching: [0; 2],
            failures: 0,
            retry_at: None,
        };
        pool.refill();
        Ok(pool)
    }

    /// Current sizes.
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Changes the idle targets (the editor keeps one windowed player warm).
    pub fn resize(&mut self, headless: usize, windowed: usize) {
        self.config.headless = headless;
        self.config.windowed = windowed;
        self.idle.retain(|p| {
            if p.hello.window {
                windowed > 0
            } else {
                headless > 0
            }
        });
        self.refill();
    }

    /// Idle players by kind.
    pub fn ready(&self, window: bool) -> usize {
        self.idle
            .iter()
            .filter(|p| p.hello.window == window)
            .count()
    }

    /// Starts launching a player on a background thread; `poll` collects it.
    fn spawn(&mut self, window: bool) -> std::io::Result<()> {
        self.next += 1;
        let name = format!(
            "{}{}",
            if window { "window" } else { "headless" },
            self.next
        );
        let log = self
            .config
            .logs
            .join(format!("player-{}-{name}.log", std::process::id()));
        let mut command = Command::new(&self.config.executable);
        command
            .arg("--owner")
            .arg(self.listener.local_addr()?.port().to_string())
            .arg("--name")
            .arg(&name)
            .arg("--gpu-cache")
            .arg(&self.config.gpu_cache)
            .arg("--size")
            .arg(format!("{}x{}", self.config.size[0], self.config.size[1]))
            .env(TOKEN_ENV, &self.token)
            .env("ARA_CRASH_DIR", self.config.logs.join("crashes"))
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        if window {
            command.arg("--window");
            if std::env::var_os("ARA_PLAY_OFFSCREEN").is_some() {
                command.arg("--offscreen");
            }
        } else if self.graphics_prewarm {
            command.arg("--prewarm-graphics");
        }
        command.arg(format!("--ray-tracing={}", self.rendering.ray_tracing));
        if let Some(scripts) = &self.scripts {
            command.env("ARA_PLAY_SCRIPTS", serde_json::to_string(scripts)?);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console; SDL still opens its own window.
            command.creation_flags(0x0800_0000);
        }
        let launches = self.launches.0.clone();
        let launch = move || {
            let result = std::fs::File::create(&log).and_then(|file| {
                command.stderr(Stdio::from(file));
                Ok(Spawning {
                    child: command.spawn()?,
                    log,
                    window,
                    spawned: Instant::now(),
                })
            });
            // The pool is gone: nobody will take this player.
            if let Err(mpsc::SendError(Launched {
                result: Ok(mut orphan),
                ..
            })) = launches.send(Launched { window, result })
            {
                let _ = orphan.child.kill();
            }
        };
        std::thread::Builder::new()
            .name("ara-player-launch".into())
            .spawn(launch)?;
        self.launching[usize::from(window)] += 1;
        Ok(())
    }

    /// After a player failed to start: no refill for 0.5 s, doubling to 30 s.
    fn back_off(&mut self) {
        self.failures += 1;
        let delay = Duration::from_millis(250)
            .saturating_mul(1 << self.failures.min(7))
            .min(Duration::from_secs(30));
        self.retry_at = Some(Instant::now() + delay);
    }

    /// Launches players until the idle targets (counting launches and spawns in
    /// flight) are met; nothing while backing off after a failed start.
    pub fn refill(&mut self) {
        if self.retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        for window in [false, true] {
            let target = if window {
                self.config.windowed
            } else {
                self.config.headless
            };
            let have = self.ready(window) + self.pending(window);
            for _ in have..target {
                if let Err(e) = self.spawn(window) {
                    eprintln!("ara-play-host: cannot spawn a player: {e}");
                    return;
                }
            }
        }
    }

    /// Accepts connections and completes hellos; reaps dead spawns. Never blocks
    /// longer than one hello exchange.
    pub fn poll(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut failed = 0;
        while let Ok(launched) = self.launches.1.try_recv() {
            self.launching[usize::from(launched.window)] -= 1;
            match launched.result {
                Ok(spawning) => self.spawning.push(spawning),
                Err(e) => {
                    problems.push(format!("cannot start a player: {e}"));
                    failed += 1;
                }
            }
        }
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => match self.handshake(stream) {
                    Ok(player) => self.idle.push(player),
                    Err(e) => problems.push(e),
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    problems.push(e.to_string());
                    break;
                }
            }
        }
        self.spawning.retain_mut(|s| match s.child.try_wait() {
            Ok(Some(status)) => {
                problems.push(format!(
                    "player exited before hello ({status}): {}",
                    tail(&s.log)
                ));
                failed += 1;
                false
            }
            _ if s.spawned.elapsed() > Duration::from_secs(30) => {
                let _ = s.child.kill();
                problems.push(format!(
                    "player did not connect within 30 s: {}",
                    tail(&s.log)
                ));
                false
            }
            _ => true,
        });
        // Idle players heartbeat too; drop dead ones so they are replaced.
        self.idle.retain_mut(|p| p.check().is_none());
        if failed > 0 {
            self.back_off();
        }
        problems
    }

    /// Players of one kind launching or spawned but not yet ready.
    fn pending(&self, window: bool) -> usize {
        self.launching[usize::from(window)]
            + self.spawning.iter().filter(|s| s.window == window).count()
    }

    fn handshake(&mut self, stream: std::net::TcpStream) -> Result<Player, String> {
        let mut link = Link::accept(stream).map_err(|e| e.to_string())?;
        let message = link
            .wait(Duration::from_secs(5))
            .map_err(|e| e.to_string())?
            .ok_or("no hello within 5 s")?;
        let Message::Request { id, method, params } = message else {
            return Err("first message must be player.hello".into());
        };
        let hello: Hello = serde_json::from_value(params).map_err(|e| e.to_string())?;
        if method != HELLO || hello.token != self.token || hello.protocol != PROTOCOL {
            let _ = link.respond(
                id,
                Err(error(
                    -32001,
                    "unauthorized or incompatible player",
                    Value::Null,
                )),
            );
            return Err("rejected a player with a wrong token or protocol".into());
        }
        let index = self
            .spawning
            .iter()
            .position(|s| s.child.id() == hello.pid)
            .ok_or("hello from a process this pool did not spawn")?;
        let spawn = self.spawning.swap_remove(index);
        self.failures = 0;
        self.retry_at = None;
        link.respond(id, Ok(json!({}))).map_err(|e| e.to_string())?;
        link.flush().map_err(|e| e.to_string())?;
        Ok(Player {
            incarnation: ara_doc::IdGen::from_entropy().asset_id("ps").to_string(),
            child: spawn.child.into(),
            link,
            hello,
            log: spawn.log,
            seen: Instant::now(),
            status: None,
            grace: Instant::now(),
            events: Vec::new(),
            embed: Vec::new(),
            replies: Default::default(),
        })
    }

    /// Takes `count` ready players of one kind (single-use), waiting up to `timeout`
    /// for spawns to connect. The host tick refills the pool behind them.
    pub fn take(
        &mut self,
        count: usize,
        window: bool,
        timeout: Duration,
    ) -> Result<Vec<Player>, String> {
        self.take_checked(count, window, timeout, || Ok(()))
    }

    pub(crate) fn take_checked(
        &mut self,
        count: usize,
        window: bool,
        timeout: Duration,
        mut check: impl FnMut() -> Result<(), &'static str>,
    ) -> Result<Vec<Player>, String> {
        let start = Instant::now();
        let mut problems = Vec::new();
        loop {
            check().map_err(str::to_owned)?;
            let observed = self.poll();
            if !observed.is_empty() {
                problems = observed;
            }
            if self.ready(window) >= count {
                break;
            }
            for _ in (self.ready(window) + self.pending(window))..count {
                self.spawn(window).map_err(|e| e.to_string())?;
            }
            if start.elapsed() > timeout {
                return Err(format!(
                    "no player became ready within {timeout:?}: {problems:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut taken = Vec::new();
        while taken.len() < count {
            let index = self
                .idle
                .iter()
                .position(|p| p.hello.window == window)
                .ok_or("pool emptied while taking")?;
            taken.push(self.idle.swap_remove(index));
        }
        // No refill here: the host tick refills after Play has started.
        Ok(taken)
    }
}
