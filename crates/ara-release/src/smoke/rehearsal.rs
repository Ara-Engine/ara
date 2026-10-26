//! Opt-in packaged release-day flow. No GitHub writes, real profiles or desktop windows.
mod server;
use super::*;

pub(crate) fn run(artifacts: &Path, older_setup: &Path, directory: &Path) -> Result<()> {
    let directory = std::path::absolute(directory)?;
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()?;
    ensure!(
        directory.starts_with(dunce::simplified(&target))
            && directory != dunce::simplified(&target),
        "rehearsal profile must be a fresh child of this checkout's target"
    );
    ensure!(!directory.exists(), "rehearsal needs a fresh directory");
    ensure!(
        directory
            .parent()
            .context("profile parent")?
            .canonicalize()?
            .starts_with(&target),
        "rehearsal parent resolves outside target; create its parent there first"
    );
    fs::create_dir_all(&directory)?;
    ensure!(
        directory.canonicalize()?.starts_with(&target),
        "rehearsal profile must be under this checkout's target"
    );
    ensure!(
        !directory.join("config").exists(),
        "rehearsal needs a fresh profile directory"
    );
    let _processes = Processes(directory.clone());
    let version = env!("CARGO_PKG_VERSION");
    let env = Environment {
        installed: directory.join(format!("local/Ara/Engines/{version}")),
        directory,
    };
    for name in ["config", "local", "roaming", "crashes", "empty-layers"] {
        fs::create_dir_all(env.directory.join(name))?;
    }
    let server = server::Server::start(artifacts, &env.directory.join("mock-github.log"))?;
    let hub = env.directory.join("local/Programs/Ara Hub");
    let expected: Value = serde_json::from_slice(&fs::read(
        artifacts.join(format!("ara-hub-{version}-windows-x64/hub.json")),
    )?)?;
    let mut report = Vec::new();
    let mut step = |name: &str, operation: &mut dyn FnMut() -> Result<()>| -> Result<()> {
        let started = Instant::now();
        let result = operation();
        report.push(json!({"step":name,"passed":result.is_ok(),"seconds":started.elapsed().as_secs_f64(),"error":result.as_ref().err().map(|e|format!("{e:#}"))}));
        support::json(
            &env.directory.join("REHEARSAL_RESULTS.json"),
            &json!({"version":version,"sourceBuild":expected["build"],"steps":report}),
        )?;
        result
    };
    step("install an older Hub on a clean profile", &mut || {
        support::run(
            env.command(older_setup)
                .arg("--destination")
                .arg(&hub)
                .args(["--silent", "--no-system-integration"]),
            &env.directory.join("old-hub-install.log"),
        )
    })?;
    step(
        "older Hub discovers, verifies and prepares the signed update",
        &mut || {
            support::run(
                env.command(hub.join("ara-hub.exe"))
                    .current_dir(&hub)
                    .args(["--rehearse-update"])
                    .arg(env.directory.join("update-discovery.json"))
                    .env("ARA_RELEASE_REPO", "Snowy7/ara")
                    .env("ARA_RELEASE_TEST_API", &server.api),
                &env.directory.join("old-hub-update.log"),
            )?;
            let discovery: Value =
                serde_json::from_slice(&fs::read(env.directory.join("update-discovery.json"))?)?;
            ensure!(
                discovery["oldBuild"] != expected["build"]
                    && discovery["selected"]["build"] == expected["build"],
                "the Hub did not select a genuinely newer executable build"
            );
            Ok(())
        },
    )?;
    step(
        "external updater swaps Hub, renders offscreen and passes startup health",
        &mut || {
            let updater: Value = serde_json::from_slice(&fs::read(
                env.directory.join("config/rehearsal-updater.json"),
            )?)?;
            ara_update::updater::wait_for_parent(pid(&updater)?)?;
            let health: Value = serde_json::from_slice(&fs::read(
                env.directory.join("config/rehearsal-health.json"),
            )?)?;
            ara_update::updater::wait_for_parent(pid(&health)?)?;
            ensure!(
                health["build"] == expected["build"],
                "updated Hub rendered a different build"
            );
            ensure!(
                !hub.with_extension("old").exists(),
                "successful update retained its backup"
            );
            ensure!(
                ara_update::sha256_file(&hub.join("ara-hub.exe"))?
                    == ara_update::sha256_file(
                        &artifacts.join(format!("ara-hub-{version}-windows-x64/ara-hub.exe"))
                    )?,
                "installed Hub bytes differ from the signed package"
            );
            Ok(())
        },
    )?;
    step(
        "updated Hub selects its current bootstrap service",
        &mut || {
            support::run(
                env.command(hub.join("ara-hub.exe"))
                    .arg("--service-check")
                    .arg(env.directory.join("updated-service.json"))
                    .env("ARA_RELEASE_REPO", "Snowy7/ara")
                    .env("ARA_RELEASE_TEST_API", &server.api),
                &env.directory.join("updated-service.log"),
            )
        },
    )?;
    let mut client = ara_rpc::Client::for_project(&env.directory.join("config/hub"))
        .context("installed Hub service discovery")?;
    step(
        "discover engine releases after the service handover",
        &mut || {
            client
                .call(
                    "hub.releases.configure",
                    json!({"repo":"Snowy7/ara","channel":"preview"}),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let available = client
                    .call("hub.engines.available", json!({"channel":"preview"}))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                ensure!(
                    available["status"]["state"] != "error",
                    "engine feed failed: {available}"
                );
                if available["checking"] == false {
                    ensure!(
                        available["releases"].as_array().is_some_and(|list| list
                            .iter()
                            .any(|release| release["version"] == version)),
                        "published alpha engine is missing: {available}"
                    );
                    support::json(&env.directory.join("available-engines.json"), &available)?;
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "engine feed did not become ready"
                );
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        },
    )?;
    step(
        "install the signed engine ZIP from the feed and register it",
        &mut || {
            client
                .call(
                    "hub.engine.install",
                    json!({"version":version,"hubVersion":version}),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let deadline = Instant::now() + Duration::from_secs(600);
            loop {
                let jobs = client
                    .call("hub.downloads", json!({}))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let job = jobs["jobs"]
                    .as_array()
                    .and_then(|jobs| jobs.iter().find(|job| job["kind"] == "engine"))
                    .context("engine transfer job")?;
                ensure!(job["state"] != "failed", "engine install failed: {job}");
                if job["state"] == "complete" {
                    support::json(&env.directory.join("engine-job.json"), job)?;
                    break;
                }
                ensure!(Instant::now() < deadline, "engine install timed out: {job}");
                thread::sleep(Duration::from_millis(100));
            }
            let engines = client
                .call("hub.engines", json!({}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let engine = engines["engines"]
                .as_array()
                .and_then(|list| {
                    list.iter()
                        .find(|engine| engine["version"] == version && engine["managed"] == true)
                })
                .with_context(|| {
                    format!("installed engine is not registered as managed: {engines}")
                })?;
            ensure!(
                fs::canonicalize(Path::new(
                    engine["executable"]
                        .as_str()
                        .context("registered CLI path")?
                ))? == fs::canonicalize(env.installed.join("ara.exe"))?,
                "service registered an executable outside the downloaded engine"
            );
            ensure!(
                env.installed.join("ara.exe").is_file(),
                "engine installed outside the isolated managed root"
            );
            let marker: Value =
                serde_json::from_slice(&fs::read(env.installed.join("engine.json"))?)?;
            ensure!(
                marker["sourceCommit"] == expected["build"]["commit"],
                "engine and Hub were packaged from different source commits"
            );
            Ok(())
        },
    )?;
    let project = env.directory.join("Starter");
    step(
        "create Starter from the downloaded engine and open through Hub",
        &mut || {
            let templates = client
                .call("hub.templates", json!({}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            support::json(&env.directory.join("templates.json"), &templates)?;
            for id in ["outpost", "showcase", "starter"] {
                let template = templates["templates"]
                    .as_array()
                    .and_then(|list| list.iter().find(|template| template["id"] == id))
                    .with_context(|| format!("downloaded engine has no {id} template"))?;
                ensure!(
                    template["engineVersion"] == version && template["setup"].is_null(),
                    "installed {id} template is incomplete or from another engine: {template}"
                );
                let thumbnail = Path::new(
                    template["thumbnail"]
                        .as_str()
                        .context("installed thumbnail")?,
                );
                ensure!(
                    thumbnail.is_file()
                        && fs::canonicalize(thumbnail)?
                            .starts_with(fs::canonicalize(&env.installed)?),
                    "{id} preview is outside the downloaded engine"
                );
            }
            let created = client.call("hub.create",json!({"name":"Rehearsal Starter","path":project,"template":"starter","engineVersion":version})).map_err(|e|anyhow::anyhow!("{e}"))?;
            support::json(&env.directory.join("created.json"), &created)?;
            let id = created["id"].as_str().context("created project id")?;
            client
                .call("hub.open", json!({"project":id,"headless":true}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            client
                .call("hub.close", json!({"project":id}))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let marker: Value =
                serde_json::from_slice(&fs::read(project.join("ara.project.json"))?)?;
            ensure!(
                marker["engineVersion"] == version,
                "sample is not pinned to the installed engine"
            );
            Ok(())
        },
    )?;
    step(
        "open the sample in the installed editor without a window",
        &mut || {
            support::run(
                env.command(env.installed.join("ara-editor.exe"))
                    .arg(&project)
                    .args(["--frames", "20", "--size", "960x600", "--output"])
                    .arg(env.directory.join("sample-editor")),
                &env.directory.join("sample-editor.log"),
            )
        },
    )?;
    step(
        "Build Current Profile and run the exported game for eight verified ticks",
        &mut || build::run(&env, &project),
    )?;
    step(
        "stop the isolated Hub service and all owned project processes",
        &mut || env.stop_hub(),
    )?;
    println!(
        "Release rehearsal passed: {}",
        env.directory.join("REHEARSAL_RESULTS.json").display()
    );
    Ok(())
}

fn pid(value: &Value) -> Result<u32> {
    Ok(value["pid"]
        .as_u64()
        .context("owned process id")?
        .try_into()?)
}

/// Cleanup is confined to executable paths beneath this unique test directory.
struct Processes(PathBuf);
impl Drop for Processes {
    fn drop(&mut self) {
        let script = self.0.join("stop-owned.ps1");
        if fs::write(&script,r#"param($Root)
$ErrorActionPreference='Stop'
$rootPath=[IO.Path]::GetFullPath($Root).TrimEnd('\')+'\'
Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($rootPath,[StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
"#).is_ok() {
            let mut command=Command::new("powershell.exe");
            command.args(["-NoProfile","-ExecutionPolicy","Bypass","-File"]).arg(script).arg(&self.0);
            #[cfg(windows)] {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let _=command.stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}
