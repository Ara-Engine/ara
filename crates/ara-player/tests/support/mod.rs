//! Shared harness for the agent-driven play tests: a temporary copy of the demo
//! project, a host with the play service, and an MCP agent session.
#![allow(dead_code, clippy::unwrap_used)]
use ara_host::{Host, HostConfig};
use ara_mcp::Adapter;
use ara_play_host::{PlayConfig, PlayService};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name == ".ara" {
            continue;
        }
        if entry.path().is_dir() {
            copy(&entry.path(), &to.join(name));
        } else {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

/// The import worker next to the player binary, built on demand.
pub fn import_worker(player: &Path) -> PathBuf {
    let worker =
        player.with_file_name(format!("ara-import-worker{}", std::env::consts::EXE_SUFFIX));
    if !worker.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "ara-import-worker"])
            .env("CARGO_BUILD_JOBS", "2")
            .status()
            .unwrap();
        assert!(status.success(), "building ara-import-worker failed");
    }
    worker
}

pub struct Agent {
    pub host: Host,
    pub mcp: Adapter,
    pub session: String,
}

impl Agent {
    pub fn tool(&mut self, name: &str, args: Value) -> Value {
        let shown = args.to_string();
        let t = Instant::now();
        let result = self.raw(name, args);
        assert_eq!(
            result["isError"],
            json!(false),
            "{name} {shown} after {:?}: {}",
            t.elapsed(),
            result["content"]
        );
        result["structuredContent"].clone()
    }
    pub fn raw(&mut self, name: &str, mut args: Value) -> Value {
        if name != "ara_session" {
            args["session"] = json!(self.session);
        }
        self.mcp
            .call_tool(&mut self.host, name, args.as_object().cloned().unwrap())
            .unwrap()
    }
    pub fn play(&mut self, action: &str, args: Value) -> Value {
        let mut args = args;
        args["action"] = json!(action);
        self.tool("ara_play", args)
    }
    /// Lets the owner run its tick (pool refill, heartbeats, imports) until `ready`.
    pub fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        let start = Instant::now();
        while !ready(self) {
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "timed out waiting for {what}"
            );
            self.host.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn pool_ready(&mut self) {
        self.until("a warm pool", |a| {
            let status = a.play("status", json!({}));
            status["pool"]["headless"].as_u64().unwrap_or(0) >= 2
        });
    }
}

impl Agent {
    /// A host on the project at `dir` (engine schemas registered) with the play
    /// service configured by `configure`, and an agent session.
    pub fn open(dir: &Path, seed: u64, configure: impl FnOnce(&mut PlayConfig)) -> Self {
        let player = PathBuf::from(env!("CARGO_BIN_EXE_ara-player"));
        let worker = import_worker(&player);
        let project = ara_play_host::open_project(dir).unwrap();
        assert!(
            project.diags.iter().all(|d| !d.is_error()),
            "{:?}",
            project.diags
        );
        let mut host = Host::open(
            project,
            HostConfig {
                seed: Some(seed),
                ..Default::default()
            },
        )
        .unwrap();
        let mut config = PlayConfig::for_project(dir);
        config.pool.executable = player;
        config.import_worker = worker;
        configure(&mut config);
        host.register(Box::new(PlayService::new(dir, config)))
            .unwrap();
        let mut agent = Agent {
            host,
            mcp: Adapter::new(),
            session: String::new(),
        };
        let begun = agent.tool(
            "ara_session",
            json!({"action": "begin", "client": "scenario", "author": "agent:scenario"}),
        );
        agent.session = begun["session"].as_str().unwrap().to_owned();
        agent
    }
}

/// A fresh temporary copy of the demo project.
pub fn demo_copy(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/demo")),
        &dir,
    );
    dir
}

/// The repository root.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds `dotnet/Ara.slnx` (Release): the JIT host, compile server and analyzers.
pub fn build_dotnet() {
    let out = std::process::Command::new("dotnet")
        .args(["build", "Ara.slnx", "-c", "Release", "--nologo", "-v", "q"])
        .current_dir(repo().join("dotnet"))
        .output()
        .expect("run `dotnet build` (is the .NET 10 SDK installed?)");
    assert!(
        out.status.success(),
        "dotnet build failed:
{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
