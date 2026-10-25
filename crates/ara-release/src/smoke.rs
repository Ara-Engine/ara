//! The shipped binaries are the test subjects; no development executable is used.
mod build;
use crate::support::{self, run};
use anyhow::{Context, Result, ensure};
use ara_proto::Endpoint;
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
        if let Some(mut client) = ara_rpc::Client::for_project(&self.directory.join("config/hub")) {
            let projects = client
                .call("hub.projects", json!({}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            for project in projects["projects"].as_array().into_iter().flatten() {
                client
                    .call("hub.close", json!({"project":project["id"]}))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            client
                .call("hub.shutdown", json!({}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while discovery.exists() {
            ensure!(Instant::now() < deadline, "smoke Hub did not stop");
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }
    fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
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
            .env("LOCALAPPDATA", self.directory.join("local"))
            .env("APPDATA", self.directory.join("roaming"))
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
pub fn smoke(dist: &Path, version: &str) -> Result<()> {
    let dist = dist.canonicalize().context("release artifacts directory")?;
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
            "standalone Hub setup, stale service replacement and engine templates",
            standalone_hub(&env, &dist, version),
        )?;
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
        let export_project = env.directory.join("Starter export");
        step(
            "create installed Starter for export",
            run(
                env.command(env.installed.join("ara.exe"))
                    .args(["hub", "create"])
                    .arg(&export_project)
                    .args(["--name", "Starter export smoke", "--template", "starter"]),
                &env.directory.join("export-create.log"),
            ),
        )?;
        step(
            "installed Build Current Profile and exported game ARA_RESULT",
            build::run(&env, &export_project),
        )?;
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
            "Ara {version}: PASS\nEvidence: {}\nSilent engine and standalone Hub SFX installs; same-version stale service replacement; standalone Hub lists outpost/showcase/starter from the installed engine and creates Starter; installed editor builds Starter with build.current, resolves the shipped player's PDB hash, and the exported game reports verified ARA_RESULT after 8 headless ticks; version; doctor without VULKAN_SDK; init; headless scene capture; native editor screenshot; every installed template opens in the editor (20 offscreen frames); global MCP project list; installed player 120 ticks; engine registration; uninstall.\nShell integration was disabled and every child used isolated config, local and roaming profile directories.\n",
            env.directory.display()
        ),
    )?;
    println!(
        "Packaged build smoke PASS; evidence: {}",
        env.directory.display()
    );
    Ok(())
}

/// Exercise the installed standalone Hub's bootstrap/handshake without opening a window.
fn standalone_hub(env: &Environment, dist: &Path, version: &str) -> Result<()> {
    let installed = env.directory.join("standalone Hub");
    run(
        env.command(dist.join(format!("ara-hub-{version}-windows-x64-setup.exe")))
            .arg("--destination")
            .arg(&installed)
            .args(["--silent", "--no-system-integration"]),
        &env.directory.join("standalone-install.log"),
    )?;
    // Preserve the real registered engine, but start a distinct same-version
    // executable. PE overlays don't change execution and do change its build hash.
    env.cli(&["hub", "engines"], "standalone-engines.log")?;
    env.stop_hub()?;
    let stale_dir = env.directory.join("local/Ara/Services").join(version);
    fs::create_dir_all(&stale_dir)?;
    let stale_exe = stale_dir.join("ara.exe");
    fs::copy(installed.join("bootstrap/ara.exe"), &stale_exe)?;
    fs::OpenOptions::new()
        .append(true)
        .open(&stale_exe)?
        .write_all(b"stale-preview-smoke")?;
    support::json(
        &stale_dir.join("service.json"),
        &json!({"version":version,"build":"stale-preview"}),
    )?;
    for entry in fs::read_dir(installed.join("bootstrap"))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        {
            fs::copy(
                &path,
                stale_dir.join(path.file_name().context("bootstrap DLL")?),
            )?;
        }
    }
    let mut stale = OwnedProcess(
        env.command(&stale_exe)
            .args(["hub", "serve"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(env.directory.join("stale-service.log"))?)
            .spawn()?,
    );
    let discovery = env.directory.join("config/hub");
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(mut client) = ara_rpc::Client::for_project(&discovery) {
            client
                .call("hub.status", json!({}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            break;
        }
        ensure!(
            stale.0.try_wait()?.is_none(),
            "seeded stale service exited before the Hub connected"
        );
        ensure!(
            Instant::now() < until,
            "seeded stale service startup timed out"
        );
        thread::sleep(Duration::from_millis(25));
    }
    let report = env.directory.join("standalone-service.json");
    run(
        env.command(installed.join("ara-hub.exe"))
            .arg("--service-check")
            .arg(&report)
            .arg("--ara")
            .arg(&stale_exe),
        &env.directory.join("standalone-service-check.log"),
    )?;
    let report: Value = serde_json::from_slice(&fs::read(report)?)?;
    let executable = PathBuf::from(report["executable"].as_str().context("bootstrap path")?);
    ensure!(
        executable.starts_with(env.directory.join("local/Ara/Services")),
        "Hub used a service outside the isolated profile: {}",
        executable.display()
    );
    ensure!(
        report["status"]["build"] == ara_update::sha256_file(&installed.join("bootstrap/ara.exe"))?,
        "Hub reused the stale service: {report}"
    );
    let info = ara_rpc::DaemonInfo::read(&discovery).context("Hub service discovery")?;
    ensure!(
        info.pid != stale.0.id(),
        "Hub retained the seeded stale service"
    );
    let templates = report["templates"].as_array().context("templates")?;
    for id in ["outpost", "showcase", "starter"] {
        let tile = templates
            .iter()
            .find(|tile| tile["id"] == id)
            .with_context(|| format!("standalone Hub didn't list {id}: {report}"))?;
        ensure!(
            tile["engineVersion"] == version,
            "template {id} did not come from the installed engine: {tile}"
        );
    }
    let mut client = ara_rpc::Client::for_project(&discovery).context("standalone service")?;
    let project = env.directory.join("Standalone Starter");
    let created = client
        .call(
            "hub.create",
            json!({"template":"starter","name":"Standalone Starter","path":project}),
        )
        .map_err(|e| anyhow::anyhow!("Starter creation failed: {e}"))?;
    ensure!(
        created["engineVersion"] == version && project.join("ara.project.json").is_file(),
        "standalone Hub Starter creation failed: {created}"
    );
    support::json(&env.directory.join("standalone-starter.json"), &created)?;
    Ok(())
}

struct OwnedProcess(Child);
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            use std::os::windows::process::CommandExt;
            // A failed export can still own a dotnet compiler/import worker.
            // Kill only this smoke-owned process tree before reaping its root.
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_smoke_child_has_isolated_config_and_local_profile() {
        let directory = std::path::absolute(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release-smoke-env-test"),
        )
        .expect("target path");
        let env = Environment {
            installed: directory.join("engine"),
            directory: directory.clone(),
        };
        let command = env.command("fixture-do-not-run");
        for (key, suffix) in [
            ("ARA_CONFIG_DIR", "config"),
            ("LOCALAPPDATA", "local"),
            ("APPDATA", "roaming"),
            ("ARA_CRASH_DIR", "crashes"),
        ] {
            let value = command
                .get_envs()
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| value)
                .expect("isolated child env");
            assert_eq!(Path::new(value), directory.join(suffix));
        }
        // The environment fixture didn't start a process; don't run cleanup.
        std::mem::forget(env);
    }
}
