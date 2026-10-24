//! The shipped binaries are the test subjects; no development executable is used.
use crate::support::{self, run};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

struct Environment {
    directory: PathBuf,
    installed: PathBuf,
}
impl Environment {
    fn stop_hub(&self) -> Result<()> {
        let discovery = self.directory.join("config/hub/.ara/daemon.json");
        if !discovery.is_file() {
            return Ok(());
        }
        let _ = self.cli(&["hub", "close", "Release smoke"], "cleanup-close.log");
        self.cli(&["hub", "shutdown"], "cleanup-shutdown.log")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while discovery.exists() {
            ensure!(Instant::now() < deadline, "smoke Hub did not stop");
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }
    fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        let paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| {
                !p.to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("vulkansdk")
            })
            .collect::<Vec<_>>();
        command
            .current_dir(&self.directory)
            .env_remove("VULKAN_SDK")
            .env_remove("SLANGC")
            .env_remove("ARA_DOTNET_ARTIFACTS")
            .env_remove("ARA_PLAYER")
            .env_remove("ARA_IMPORT_WORKER")
            .env(
                "VK_IMPLICIT_LAYER_PATH",
                self.directory.join("empty-layers"),
            )
            .env("PATH", std::env::join_paths(paths).unwrap_or_default())
            .env("ARA_CONFIG_DIR", self.directory.join("config"))
            .env("GIT_CEILING_DIRECTORIES", &self.directory)
            .env("ARA_CRASH_DIR", self.directory.join("crashes"));
        command
    }
    fn cli(&self, args: &[&str], log: &str) -> Result<()> {
        run(
            self.command(self.installed.join("ara.exe")).args(args),
            &self.directory.join(log),
        )
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        if let Err(error) = self.stop_hub() {
            eprintln!("release smoke cleanup failed: {error:#}");
        }
    }
}
pub fn smoke(root: &Path, version: &str) -> Result<()> {
    let dist = support::dist(root);
    let directory = dist.join(format!(
        "smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    ));
    fs::create_dir(&directory)?;
    fs::create_dir(directory.join("empty-layers"))?;
    let env = Environment {
        installed: directory.join("installed engine"),
        directory,
    };
    let mut report = Vec::new();
    let mut step = |name: &str, result: Result<()>| -> Result<()> {
        report.push(json!({"step":name,"passed":result.is_ok(),"error":result.as_ref().err().map(|e| format!("{e:#}"))}));
        support::json(
            &dist.join("SMOKE_RESULTS.json"),
            &json!({"version":version,"directory":env.directory,"steps":report}),
        )?;
        result
    };
    step(
        "silent SFX install",
        run(
            env.command(dist.join(format!("{}-setup.exe", support::name(version))))
                .arg("--destination")
                .arg(&env.installed)
                .args(["--silent", "--no-system-integration"]),
            &env.directory.join("install.log"),
        ),
    )?;
    let result = (|| -> Result<()> {
        step(
            "ara --version",
            env.cli(&["--version"], "version.log").and_then(|()| {
                ensure!(
                    fs::read_to_string(env.directory.join("version.log"))?.trim()
                        == format!("ara {version}"),
                    "installed version mismatch"
                );
                Ok(())
            }),
        )?;
        step(
            "ara doctor (bundled tools, no SDK)",
            env.cli(&["doctor"], "doctor.log"),
        )?;
        step(
            "bundled Slang + DXC + SPIR-V shader cook",
            (|| {
                let source = env.directory.join("shader-source");
                fs::create_dir(&source)?;
                fs::write(
                    source.join("test.slang"),
                    "RWStructuredBuffer<float> result : register(u0); [shader(\"compute\")] [numthreads(1,1,1)] void computeMain(uint3 t:SV_DispatchThreadID) { result[t.x]=1.; }",
                )?;
                support::json(
                    &source.join("pipelines.json"),
                    &json!({"test":{"source":"test.slang","entries":{"computeMain":"compute"},"state":{"color_formats":[],"depth_format":null,"vertex_stride":0,"depth_test":false,"depth_write":false,"blend":false}}}),
                )?;
                run(
                    env.command(env.installed.join("ara-shader-cook.exe"))
                        .arg(source.join("pipelines.json"))
                        .arg(env.directory.join("shader-cooked")),
                    &env.directory.join("shader-cook.log"),
                )?;
                ensure!(
                    env.directory.join("shader-cooked/test.json").is_file(),
                    "shader artifact missing"
                );
                Ok(())
            })(),
        )?;
        let project = env.directory.join("starter project");
        step(
            "ara init",
            run(
                env.command(env.installed.join("ara.exe"))
                    .arg("init")
                    .arg(&project)
                    .args(["--name", "Release smoke"]),
                &env.directory.join("init.log"),
            ),
        )?;
        step(
            "ara-editor headless capture",
            run(
                env.command(env.installed.join("ara-editor.exe"))
                    .arg(&project)
                    .args(["--headless", "--sample", "--output"])
                    .arg(env.directory.join("capture")),
                &env.directory.join("editor-headless.log"),
            )
            .and_then(|()| {
                ensure!(
                    env.directory.join("capture/editor-scene.png").is_file(),
                    "editor headless capture missing"
                );
                Ok(())
            }),
        )?;
        step(
            "ara-editor native window capture",
            run(
                env.command(env.installed.join("ara-editor.exe"))
                    .arg(&project)
                    .args(["--frames", "4", "--screenshot", "--output"])
                    .arg(env.directory.join("native")),
                &env.directory.join("editor-native.log"),
            )
            .and_then(|()| {
                ensure!(
                    env.directory.join("native/editor.png").is_file(),
                    "native editor capture missing"
                );
                Ok(())
            }),
        )?;
        // Every shipped template opens in the installed editor (offscreen frames).
        let mut templates = fs::read_dir(env.installed.join("templates"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        templates.sort();
        for template in templates
            .iter()
            .filter(|path| path.join("ara.project.json").is_file())
        {
            let name = template
                .file_name()
                .context("template directory")?
                .to_string_lossy();
            let copy = env.directory.join("templates").join(name.as_ref());
            step(
                &format!("ara-editor opens template {name}"),
                support::copy_tree(template, &copy).and_then(|()| {
                    run(
                        env.command(env.installed.join("ara-editor.exe"))
                            .arg(&copy)
                            .args(["--frames", "20"]),
                        &env.directory.join(format!("template-{name}.log")),
                    )
                }),
            )?;
        }
        let hub = env.installed.join("ara-hub.exe");
        if hub.is_file() {
            step(
                "ara-hub native capture (every page, both themes)",
                run(
                    env.command(&hub)
                        .arg("--capture")
                        .arg(env.directory.join("hub"))
                        .args(["--size", "1440x900", "--dpi", "1"]),
                    &env.directory.join("hub-capture.log"),
                )
                .and_then(|()| {
                    ensure!(
                        env.directory
                            .join("hub/1440x900/dark-projects.png")
                            .is_file(),
                        "installed Hub capture missing"
                    );
                    Ok(())
                }),
            )?;
        }
        step(
            "Hub register project",
            run(
                env.command(env.installed.join("ara.exe"))
                    .args(["hub", "add"])
                    .arg(&project),
                &env.directory.join("hub-add.log"),
            ),
        )?;
        let mut mcp = Mcp::start(&env)?;
        step(
            "global MCP ara_projects list",
            (|| {
                let list = mcp.tool("ara_projects", json!({"action":"list"}))?;
                ensure!(
                    list["projects"].as_array().is_some_and(|projects| projects
                        .iter()
                        .any(|p| p["name"] == "Release smoke")),
                    "global MCP did not list installed-build project: {list}"
                );
                Ok(())
            })(),
        )?;
        step(
            "ara-player headless 120 ticks",
            (|| {
                mcp.tool(
                    "ara_session",
                    json!({"action":"open","project":"Release smoke","client":"release-smoke"}),
                )?;
                let start = mcp.tool(
                    "ara_play",
                    json!({"action":"start","startPaused":true,"window":false,"seed":7}),
                )?;
                ensure!(
                    start["players"]
                        .as_array()
                        .is_some_and(|players| !players.is_empty()),
                    "no installed player started: {start}"
                );
                let stepped = mcp.tool("ara_play", json!({"action":"step","ticks":120}))?;
                ensure!(
                    stepped["tick"].as_u64() == Some(120),
                    "player did not reach tick 120: {stepped}"
                );
                mcp.tool("ara_play", json!({"action":"stop"}))?;
                mcp.tool(
                    "ara_projects",
                    json!({"action":"close","project":"Release smoke"}),
                )?;
                Ok(())
            })(),
        )?;
        drop(mcp);
        step(
            "Hub engine registration",
            env.cli(&["hub", "engines"], "engines.log").and_then(|()| {
                let engines: Value =
                    serde_json::from_slice(&fs::read(env.directory.join("engines.log"))?)?;
                ensure!(
                    engines["engines"]
                        .as_array()
                        .is_some_and(|engines| engines.iter().any(|e| e["version"] == version
                            && e["executable"]
                                .as_str()
                                .is_some_and(|p| Path::new(p).starts_with(&env.installed)))),
                    "installed engine missing from Hub"
                );
                Ok(())
            }),
        )?;
        Ok(())
    })();
    // Best effort close on failure too; never leave a test user Hub/player running.
    let _ = env.cli(&["hub", "close", "Release smoke"], "close.log");
    let uninstall = run(
        env.command("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(env.installed.join("uninstall.ps1"))
            .arg("-Silent"),
        &env.directory.join("uninstall.log"),
    )
    .and_then(|()| {
        ensure!(
            !env.installed.exists(),
            "uninstaller left installation files"
        );
        Ok(())
    });
    step("uninstall", uninstall)?;
    result?;
    fs::write(
        dist.join("SMOKE_RESULTS.txt"),
        format!(
            "Ara {version}: PASS\nEvidence: {}\nSilent SFX install; version; doctor without VULKAN_SDK; init; headless scene capture; native editor screenshot; every installed template opens in the editor (20 offscreen frames); global MCP project list; installed player 120 ticks; engine registration; uninstall.\nShell integration was disabled to keep the real user's PATH and Start menu unchanged.\n",
            env.directory.display()
        ),
    )?;
    println!(
        "Packaged build smoke PASS; evidence: {}",
        env.directory.display()
    );
    Ok(())
}

struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    messages: Receiver<String>,
    log: fs::File,
    next: u64,
}
impl Mcp {
    fn start(env: &Environment) -> Result<Self> {
        let mut child = env
            .command(env.installed.join("ara.exe"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(fs::File::create(
                env.directory.join("mcp-stderr.log"),
            )?))
            .spawn()?;
        let input = child.stdin.take().context("MCP stdin")?;
        let output = child.stdout.take().context("MCP stdout")?;
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut result = Self {
            child,
            input: Some(input),
            messages,
            log: fs::File::create(env.directory.join("mcp-transcript.jsonl"))?,
            next: 0,
        };
        result.call("initialize", json!({"protocolVersion":"2025-11-25","clientInfo":{"name":"release-smoke","version":"1"},"capabilities":{}}))?;
        Ok(result)
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let request = json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params});
        writeln!(self.log, "{request}")?;
        let input = self.input.as_mut().context("MCP stdin closed")?;
        writeln!(input, "{request}")?;
        input.flush()?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let line = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("MCP response timed out or stdout closed")?;
            writeln!(self.log, "{line}")?;
            let message: Value = serde_json::from_str(&line)?;
            if message["id"] == self.next {
                ensure!(message.get("error").is_none(), "MCP RPC error: {message}");
                return Ok(message["result"].clone());
            }
        }
    }
    fn tool(&mut self, name: &str, args: Value) -> Result<Value> {
        let result = self.call("tools/call", json!({"name":name,"arguments":args}))?;
        ensure!(result["isError"] != true, "{name} failed: {result}");
        Ok(result["structuredContent"].clone())
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
