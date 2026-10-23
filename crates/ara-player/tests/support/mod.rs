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
        Self::open_with(dir, seed, configure, |_| {})
    }

    /// As [`Agent::open`], with `service` run on the play service before the host
    /// takes it (the editor takes its embedded-surface handle there).
    pub fn open_with(
        dir: &Path,
        seed: u64,
        configure: impl FnOnce(&mut PlayConfig),
        service: impl FnOnce(&mut PlayService),
    ) -> Self {
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
        if let Some(player) = std::env::var_os("ARA_BENCH_PLAYER") {
            config.pool.executable = player.into();
        }
        config.import_worker = worker;
        configure(&mut config);
        let mut play = PlayService::new(dir, config);
        service(&mut play);
        host.register(Box::new(play)).unwrap();
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

/// Posts the real native window-close event to this harness's player only.
#[cfg(windows)]
pub fn close_window(pid: u32) -> bool {
    struct Target {
        pid: u32,
        posted: bool,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            parameter: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: isize, pid: *mut u32) -> u32;
        fn IsWindowVisible(window: isize) -> i32;
        fn PostMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> i32;
    }
    unsafe extern "system" fn visit(window: isize, parameter: isize) -> i32 {
        // SAFETY: EnumWindows invokes synchronously with our live Target pointer.
        let target = unsafe { &mut *(parameter as *mut Target) };
        let mut pid = 0;
        // SAFETY: the OS supplies a window handle; the output PID pointer is valid.
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
            if pid == target.pid && IsWindowVisible(window) != 0 {
                target.posted = PostMessageW(window, 0x0010, 0, 0) != 0;
                return 0;
            }
        }
        1
    }
    let mut target = Target { pid, posted: false };
    // SAFETY: visit has the Win32 callback ABI and Target outlives this enumeration.
    unsafe {
        EnumWindows(visit, std::ptr::from_mut(&mut target) as isize);
    }
    target.posted
}
