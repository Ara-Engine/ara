//! Pre-spawned, single-use `ara-player` processes (S5): spawned ahead of Play,
//! authenticated by the per-session token in `player.hello`, idle at zero CPU until
//! taken. Faults are classified from the exit code, the stderr crash line and the
//! 250 ms heartbeat (a 1 s silence is a hang; the player is killed).
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
    time::{Duration, Instant},
};

/// How long a running player may stay silent before it is declared hung.
pub const WATCHDOG: Duration = Duration::from_secs(1);

/// A connected player process.
pub struct Player {
    /// Owned child; killed on drop.
    pub child: Child,
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
        if let Message::Notification { method, params } = message {
            if method == STATUS {
                self.status = serde_json::from_value(params.clone()).ok();
            } else if method == BUSY
                && let Ok(busy) = serde_json::from_value::<Busy>(params.clone())
            {
                self.grace = Instant::now() + Duration::from_millis(busy.max_ms.min(300_000));
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
        let id = match self.link.request(method, params) {
            Ok(id) => json!(id),
            Err(_) => return Err(self.closed()),
        };
        let started = Instant::now();
        self.seen = Instant::now();
        loop {
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
            // Busy players keep heartbeating between tick chunks; silence is a hang.
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
        loop {
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
}

impl Pool {
    /// Binds the live-link listener on 127.0.0.1 and starts filling the pool.
    pub fn new(config: PoolConfig, token: String) -> std::io::Result<Self> {
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
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::create(&log)?));
        if window {
            command.arg("--window");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console; SDL still opens its own window.
            command.creation_flags(0x0800_0000);
        }
        self.spawning.push(Spawning {
            child: command.spawn()?,
            log,
            window,
            spawned: Instant::now(),
        });
        Ok(())
    }

    /// Spawns players until the idle targets (counting in-flight spawns) are met.
    pub fn refill(&mut self) {
        for window in [false, true] {
            let target = if window {
                self.config.windowed
            } else {
                self.config.headless
            };
            let have =
                self.ready(window) + self.spawning.iter().filter(|s| s.window == window).count();
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
        problems
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
        link.respond(id, Ok(json!({}))).map_err(|e| e.to_string())?;
        link.flush().map_err(|e| e.to_string())?;
        Ok(Player {
            child: spawn.child,
            link,
            hello,
            log: spawn.log,
            seen: Instant::now(),
            status: None,
            grace: Instant::now(),
        })
    }

    /// Takes `count` ready players of one kind (single-use), waiting up to `timeout`
    /// for spawns to connect, then refills behind them.
    pub fn take(
        &mut self,
        count: usize,
        window: bool,
        timeout: Duration,
    ) -> Result<Vec<Player>, String> {
        let start = Instant::now();
        loop {
            let problems = self.poll();
            if self.ready(window) >= count {
                break;
            }
            let pending = self.spawning.iter().filter(|s| s.window == window).count();
            for _ in (self.ready(window) + pending)..count {
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
        self.refill();
        Ok(taken)
    }
}
