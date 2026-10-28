//! `play`: Play captures through the engine's agent interface. Starts `ara mcp` in
//! the project (it becomes the daemon), then uses the `ara_play` tool exactly as an
//! agent would: start a pooled `ara-player` (windowed or headless), walk the player,
//! capture what the window presents and the full raster capture (terrain, foliage
//! stand-ins and VFX composited), read status, stop.
use crate::build::tool;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

/// An MCP client over `ara mcp`'s stdio.
pub(crate) struct Mcp {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    pub(crate) fn start(root: &Path) -> Result<Self> {
        let ara = tool("ara", "ARA_BIN")?;
        let log = std::fs::File::create(root.join(".ara/samples/ara-mcp.log"))?;
        let mut child = Command::new(ara)
            .arg("--root")
            .arg(root)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn()
            .context("starting ara mcp")?;
        let input = child.stdin.take().context("mcp stdin")?;
        let output = BufReader::new(child.stdout.take().context("mcp stdout")?);
        let mut mcp = Self {
            child,
            input,
            output,
            next: 1,
        };
        mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "ara-samples", "version": "0"}}),
        )?;
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(mcp)
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        writeln!(self.input, "{message}")?;
        self.input.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let mut line = String::new();
            if self.output.read_line(&mut line)? == 0 {
                bail!("ara mcp exited (see .ara/samples/ara-mcp.log)");
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == json!(id) {
                if !message["error"].is_null() {
                    bail!("{method}: {}", message["error"]);
                }
                return Ok(message["result"].clone());
            }
        }
    }

    /// Calls `ara_play` with `action`, failing on a tool error.
    pub(crate) fn play(&mut self, action: &str, mut args: Value) -> Result<Value> {
        args["action"] = json!(action);
        let result = self.request("tools/call", json!({"name": "ara_play", "arguments": args}))?;
        if result["isError"] == json!(true) {
            bail!("ara_play {action}: {}", result["content"]);
        }
        if std::env::var_os("ARA_SAMPLES_TRACE").is_some() {
            eprintln!("ara_play {action} -> {result}");
        }
        Ok(result["structuredContent"].clone())
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What to capture.
pub struct Request<'a> {
    /// Project folder under `samples/`.
    pub project: &'a str,
    /// Scene path in the project.
    pub scene: &'a str,
    /// Capture file stem.
    pub name: &'a str,
    /// Ticks to walk forward before capturing (0: stand still).
    pub walk: u32,
    /// A windowed player (else headless).
    pub window: bool,
}

/// Plays `scene` windowed, walks, captures (presented and full) and reports status.
pub fn run(samples: &Path, request: &Request<'_>) -> Result<Value> {
    let root = samples.join(request.project);
    std::fs::create_dir_all(root.join(".ara/samples"))?;
    let evidence = crate::evidence_directory(request.project);
    std::fs::create_dir_all(&evidence)?;
    let mut mcp = Mcp::start(&root)?;
    let started = Instant::now();
    let start = mcp.play(
        "start",
        json!({"scene": request.scene, "window": request.window, "seed": 7}),
    )?;
    let world = start["world"].clone();
    eprintln!(
        "play.start: {} ms boot, {} ms prepare",
        start["bootMs"], start["prepareMs"]
    );
    let status = |mcp: &mut Mcp| -> Result<Value> {
        let s = mcp.play("status", json!({}))?;
        let world = &s["worlds"][0];
        if !world["fault"].is_null() {
            bail!("world fault: {}", world["fault"]);
        }
        Ok(world["players"][0]["status"].clone())
    };
    wait(
        &mut mcp,
        &status,
        |s| s["tick"].as_u64().unwrap_or(0) >= 60,
        "the first second of play",
    )?;
    if request.walk > 0 {
        mcp.play(
            "input",
            json!({"world": world, "inputs": [{"action": "Move", "value": [0, 1], "holdTicks": request.walk}]}),
        )?;
        let begin = status(&mut mcp)?["tick"].as_u64().unwrap_or(0);
        let target = begin + u64::from(request.walk) / 2;
        wait(
            &mut mcp,
            &status,
            |s| s["tick"].as_u64().unwrap_or(0) >= target,
            "walking",
        )?;
    }
    let path = |suffix: &str| evidence.join(format!("{}-{suffix}.png", request.name));
    let presented = mcp.play(
        "capture",
        json!({"world": world, "path": path("player-window").to_string_lossy(), "width": 1280, "height": 720, "marks": 0, "presented": true}),
    )?;
    let full = mcp.play(
        "capture",
        json!({"world": world, "path": path("player").to_string_lossy(), "width": 1280, "height": 720, "marks": 0}),
    )?;
    // Let the window present for a while, then read its frame statistics.
    std::thread::sleep(Duration::from_secs(4));
    let last = status(&mut mcp)?;
    mcp.play("stop", json!({"world": world}))?;
    for marks in ["player-window.marks.png", "player.marks.png"] {
        let _ = std::fs::remove_file(evidence.join(format!("{}-{marks}", request.name)));
    }
    Ok(json!({
        "scene": request.scene,
        "start": start,
        "presentedCapture": presented,
        "fullCapture": full,
        "status": last,
        "wallSeconds": started.elapsed().as_secs_f64(),
    }))
}

fn wait(
    mcp: &mut Mcp,
    status: &dyn Fn(&mut Mcp) -> Result<Value>,
    ready: impl Fn(&Value) -> bool,
    what: &str,
) -> Result<()> {
    let start = Instant::now();
    loop {
        let s = status(mcp)?;
        if ready(&s) {
            return Ok(());
        }
        if !s["fault"].is_null() && s["fault"] != json!({}) {
            bail!("player fault while waiting for {what}: {}", s["fault"]);
        }
        if start.elapsed() > Duration::from_secs(300) {
            bail!("timed out waiting for {what}: {s}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
