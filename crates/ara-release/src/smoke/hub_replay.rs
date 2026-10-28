//! Installed Hub update proof from prebuilt binaries; no build, real profile or window.
use super::*;
use ara_update::{Asset, BuildIdentity, Channel, Kind, Manifest};

struct Package {
    directory: PathBuf,
    archive: PathBuf,
    setup: PathBuf,
    info: Value,
}

pub(crate) fn run(
    older_bins: &Path,
    newer_bins: &Path,
    key: &Path,
    directory: &Path,
) -> Result<()> {
    ensure!(cfg!(windows), "installed Hub replay requires Windows");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let target = root.join("target").canonicalize()?;
    let directory = std::path::absolute(directory)?;
    ensure!(
        !directory.exists()
            && directory
                .parent()
                .context("replay parent")?
                .canonicalize()?
                .starts_with(&target),
        "Hub replay needs a fresh directory below this checkout's target; create its parent first"
    );
    let key = key
        .canonicalize()
        .context("external throwaway signing key")?;
    ensure!(
        !key.starts_with(&root),
        "private signing keys must be outside the repository"
    );
    let older_bins = older_bins
        .canonicalize()
        .context("older binary directory")?;
    let newer_bins = newer_bins
        .canonicalize()
        .context("newer binary directory")?;
    fs::create_dir(&directory)?;
    ensure!(
        directory.canonicalize()?.starts_with(&target),
        "Hub replay resolved outside target"
    );
    let env = Environment {
        installed: directory.join("local/Programs/Ara Hub"),
        directory,
    };
    for name in [
        "config",
        "local",
        "roaming",
        "crashes",
        "empty-layers",
        "packages",
    ] {
        fs::create_dir_all(env.directory.join(name))?;
    }
    let _cleanup = Detached(&env);
    let mut report = json!({"format":"ara.hub.replay/1","directory":env.directory,
        "olderBins":older_bins,"newerBins":newer_bins,"steps":[],"passed":false});
    let result = replay(&env, &root, &older_bins, &newer_bins, &key, &mut report);
    report["passed"] = json!(result.is_ok());
    report["error"] = json!(result.as_ref().err().map(|error| format!("{error:#}")));
    support::json(&env.directory.join("HUB_REPLAY_RESULTS.json"), &report)?;
    result?;
    println!(
        "Installed Hub replay passed: {}",
        env.directory.join("HUB_REPLAY_RESULTS.json").display()
    );
    Ok(())
}

fn replay(
    env: &Environment,
    root: &Path,
    older: &Path,
    newer: &Path,
    key: &Path,
    report: &mut Value,
) -> Result<()> {
    let (old_info, new_info) = step(
        env,
        report,
        "read the actual old/new binary identities",
        || {
            let old = build_info(env, &older.join("ara-hub.exe"), "older-build-info")?;
            let new = build_info(env, &newer.join("ara-hub.exe"), "newer-build-info")?;
            let old_build: BuildIdentity = serde_json::from_value(old["build"].clone())?;
            let new_build: BuildIdentity = serde_json::from_value(new["build"].clone())?;
            let old_version = old["version"].as_str().context("older Hub version")?;
            let new_version = new["version"].as_str().context("newer Hub version")?;
            ensure!(
                ara_update::build::newer(
                    new_version,
                    Some(&new_build),
                    old_version,
                    Some(&old_build)
                ),
                "newer binaries must have a genuinely newer version/source generation"
            );
            ensure!(
                old_build.commit != new_build.commit
                    && ara_update::sha256_file(&older.join("ara-hub.exe"))?
                        != ara_update::sha256_file(&newer.join("ara-hub.exe"))?,
                "old and new must be distinct actual builds"
            );
            Ok((old, new))
        },
    )?;
    report["olderIdentity"] = old_info.clone();
    report["newerIdentity"] = new_info.clone();
    let (old, new) = step(
        env,
        report,
        "stage minimal standalone packages without invoking Cargo",
        || {
            let setup = newer.join("ara-setup.exe");
            ensure!(
                setup.is_file(),
                "newer-bins must include the prebuilt ara-setup.exe"
            );
            Ok((
                stage(env, root, older, &setup, "older", old_info)?,
                stage(env, root, newer, &setup, "newer", new_info)?,
            ))
        },
    )?;
    report["payloads"] = json!({"older":{"zip":old.archive,"sha256":ara_update::sha256_file(&old.archive)?,
        "hubSha256":ara_update::sha256_file(&old.directory.join("ara-hub.exe"))?,
        "serviceSha256":ara_update::sha256_file(&old.directory.join("bootstrap/ara.exe"))?},
        "newer":{"zip":new.archive,"sha256":ara_update::sha256_file(&new.archive)?,
        "hubSha256":ara_update::sha256_file(&new.directory.join("ara-hub.exe"))?,
        "serviceSha256":ara_update::sha256_file(&new.directory.join("bootstrap/ara.exe"))?}});
    let artifacts = step(
        env,
        report,
        "sign the exact new portable ZIP using the external disposable key",
        || signed_feed(env, &new, key),
    )?;
    let version = new.info["version"].as_str().context("newer version")?;
    let server = super::rehearsal::server::Server::start_hub(
        &artifacts,
        version,
        &env.directory.join("mock-github.log"),
    )?;
    step(
        env,
        report,
        "silently install the actual older Hub into the isolated profile",
        || {
            support::run(
                env.command(&old.setup)
                    .arg("--destination")
                    .arg(&env.installed)
                    .args(["--silent", "--no-system-integration"]),
                &env.directory.join("older-install.log"),
            )?;
            ensure!(
                !env.directory.join("config/hub/.ara/daemon.json").exists(),
                "silent setup unexpectedly started the service"
            );
            ensure!(
                ara_update::sha256_file(&env.installed.join("ara-hub.exe"))?
                    == ara_update::sha256_file(&old.directory.join("ara-hub.exe"))?,
                "setup did not install the older binary"
            );
            Ok(())
        },
    )?;
    let prefs = env.directory.join("config/hub-ui.json");
    support::json(
        &prefs,
        &json!({"light":true,"sort":"name","layout":"list","agent":"codex","hubUpdateDownloads":"ask"}),
    )?;
    let saved_prefs = fs::read(&prefs)?;
    let ready = env.directory.join("update-discovery.json");
    let gate = env.directory.join("config/restart-consent");
    let log = fs::File::create(env.directory.join("older-update.log"))?;
    let mut command = env.command(env.installed.join("ara-hub.exe"));
    command
        .arg("--rehearse-update")
        .arg(&ready)
        .env("ARA_RELEASE_REPO", "Ara-Engine/ara")
        .env("ARA_RELEASE_TEST_API", &server.api)
        .env("ARA_RELEASE_REHEARSAL_RESTART_GATE", &gate)
        .env_remove("GITHUB_TOKEN")
        .env_remove("ARA_RELEASE_SIGNING_KEY")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    let mut older_hub = OwnedProcess(
        command
            .spawn()
            .context("start installed older Hub offscreen")?,
    );
    step(
        env,
        report,
        "discover/download/verify and prove no apply before explicit restart",
        || {
            let proof = wait_ready(&mut older_hub, &ready)?;
            ensure!(
                proof["oldPid"].as_u64() == Some(u64::from(older_hub.0.id()))
                    && proof["oldBuild"] == old.info["build"]
                    && proof["selected"]["build"] == new.info["build"],
                "ready proof did not identify the actual old/new builds"
            );
            ensure!(
                proof["downloadPolicy"] == "ask"
                    && proof["metadataDidNotDownload"] == true
                    && proof["downloadIntent"] == "PrepareUpdate"
                    && proof["noApplyBeforeRestart"] == true
                    && proof["restartRequested"] == false
                    && proof["restartGate"] == json!(gate)
                    && proof["metadataChecks"]
                        .as_u64()
                        .is_some_and(|count| count > 0)
                    && !proof["prepared"].is_null(),
                "ready proof does not demonstrate explicit download/restart policy"
            );
            ensure!(
                older_hub.0.try_wait()?.is_none()
                    && !gate.exists()
                    && !env.directory.join("config/rehearsal-updater.json").exists()
                    && !env.directory.join("config/updater").exists(),
                "an updater/restart happened before the restart action"
            );
            ensure!(
                ara_update::sha256_file(&env.installed.join("ara-hub.exe"))?
                    == ara_update::sha256_file(&old.directory.join("ara-hub.exe"))?,
                "verified-ready silently replaced the installed Hub"
            );
            let observe_until = Instant::now() + Duration::from_millis(750);
            while Instant::now() < observe_until {
                ensure!(
                    older_hub.0.try_wait()?.is_none()
                        && !env.directory.join("config/rehearsal-updater.json").exists()
                        && !env.directory.join("config/updater").exists(),
                    "verified-ready did not remain paused for explicit consent"
                );
                thread::sleep(Duration::from_millis(25));
            }
            ensure!(
                ara_update::sha256_file(&env.installed.join("ara-hub.exe"))?
                    == ara_update::sha256_file(&old.directory.join("ara-hub.exe"))?,
                "installed Hub changed while consent was withheld"
            );
            support::json(&env.directory.join("ready-before-consent.json"), &proof)?;
            Ok(())
        },
    )?;
    step(
        env,
        report,
        "express RestartUpdate and wait for the real external updater and health frame",
        || {
            fs::write(&gate, b"restart")?;
            wait_child(&mut older_hub, Duration::from_secs(120))?;
            let updater = wait_json(
                &env.directory.join("config/rehearsal-updater.json"),
                Duration::from_secs(30),
            )?;
            ara_update::updater::wait_for_parent(pid(&updater)?)?;
            let health = wait_json(
                &env.directory.join("config/rehearsal-health.json"),
                Duration::from_secs(30),
            )?;
            ara_update::updater::wait_for_parent(pid(&health)?)?;
            ensure!(
                health["build"] == new.info["build"],
                "startup health rendered a different build"
            );
            let proof = wait_json(&ready, Duration::from_secs(5))?;
            ensure!(
                proof["restartRequested"] == true && proof["restartIntent"] == "RestartUpdate",
                "restart action was not recorded"
            );
            ensure!(
                !env.installed.with_extension("old").exists(),
                "successful updater retained its rollback backup"
            );
            ensure!(
                ara_update::sha256_file(&env.installed.join("ara-hub.exe"))?
                    == ara_update::sha256_file(&new.directory.join("ara-hub.exe"))?,
                "installed bytes differ from the signed new ZIP"
            );
            ensure!(
                fs::read(&prefs)? == saved_prefs,
                "Hub settings changed during the update"
            );
            Ok(())
        },
    )?;
    step(
        env,
        report,
        "verify installed identity and reconnect to the new bootstrap service",
        || {
            let actual = build_info(
                env,
                &env.installed.join("ara-hub.exe"),
                "installed-build-info",
            )?;
            ensure!(
                actual == new.info,
                "installed binary identity differs from the new source build"
            );
            let service_report = env.directory.join("updated-service.json");
            support::run(
                env.command(env.installed.join("ara-hub.exe"))
                    .arg("--service-check")
                    .arg(&service_report)
                    .env("ARA_RELEASE_REPO", "Ara-Engine/ara")
                    .env("ARA_RELEASE_TEST_API", &server.api)
                    .env_remove("GITHUB_TOKEN"),
                &env.directory.join("updated-service.log"),
            )?;
            let service = wait_json(&service_report, Duration::from_secs(5))?;
            let executable = Path::new(
                service["executable"]
                    .as_str()
                    .context("service executable")?,
            )
            .canonicalize()?;
            ensure!(
                executable.starts_with(env.directory.join("local/Ara/Services").canonicalize()?)
                    && service["status"]["build"]
                        == ara_update::sha256_file(&new.directory.join("bootstrap/ara.exe"))?,
                "updated Hub did not reconnect to its own isolated bootstrap service"
            );
            ensure!(
                ara_update::sha256_file(&executable)?
                    == service["status"]["build"]
                        .as_str()
                        .context("service build")?,
                "service handshake hash differs from its executable"
            );
            ensure!(
                fs::read(&prefs)? == saved_prefs,
                "service reconnect changed Hub settings"
            );
            Ok(())
        },
    )?;
    let image = env.directory.join("config/rehearsal-updated-hub.png");
    let png = fs::read(&image).context("real offscreen startup-health capture")?;
    ensure!(
        png.len() >= 24 && png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "startup-health capture is not a PNG"
    );
    report["capture"] = json!({"path":image,"sha256":ara_update::sha256_file(&image)?,"bytes":png.len(),
        "width":u32::from_be_bytes(png[16..20].try_into()?),"height":u32::from_be_bytes(png[20..24].try_into()?),
        "sourceBuild":new.info["build"],"health":env.directory.join("config/rehearsal-health.json")});
    step(
        env,
        report,
        "stop the isolated service and reap the owned Hub processes",
        || env.stop_hub(),
    )?;
    Ok(())
}

fn build_info(env: &Environment, executable: &Path, name: &str) -> Result<Value> {
    let output = env.directory.join(format!("{name}.json"));
    support::run(
        env.command(executable).arg("--build-info").arg(&output),
        &env.directory.join(format!("{name}.log")),
    )?;
    let value: Value = serde_json::from_slice(&fs::read(output)?)?;
    ara_update::version(value["version"].as_str().context("Hub binary version")?)?;
    let build: BuildIdentity = serde_json::from_value(value["build"].clone())?;
    ensure!(
        build.commit.len() == 40 && build.commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "binary has no full source commit"
    );
    Ok(value)
}

fn stage(
    env: &Environment,
    root: &Path,
    bins: &Path,
    setup: &Path,
    label: &str,
    info: Value,
) -> Result<Package> {
    let base = env.directory.join("packages").join(label);
    let directory = base.join("hub");
    fs::create_dir_all(&directory)?;
    for binary in ["ara-hub.exe", "ara-hub-updater.exe"] {
        support::copy(&bins.join(binary), &directory.join(binary))?;
    }
    let cli = if bins.join("bootstrap/ara.exe").is_file() {
        bins.join("bootstrap/ara.exe")
    } else {
        bins.join("ara.exe")
    };
    support::copy(&cli, &directory.join("bootstrap/ara.exe"))?;
    support::json(
        &directory.join("bootstrap/service.json"),
        &json!({"version":info["version"],"build":ara_update::sha256_file(&cli)?}),
    )?;
    for source in [bins.to_owned(), bins.join("bootstrap")] {
        if !source.is_dir() {
            continue;
        }
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
            {
                support::copy(&entry.path(), &directory.join(entry.file_name()))?;
                support::copy(
                    &entry.path(),
                    &directory.join("bootstrap").join(entry.file_name()),
                )?;
            }
        }
    }
    support::copy_tree(&root.join("assets/brand"), &directory.join("assets/brand"))?;
    for name in ["LICENSE", "paths.ps1", "hub-installation.ps1"] {
        support::copy(&root.join("release").join(name), &directory.join(name))?;
    }
    fs::write(
        directory.join("install.ps1"),
        include_bytes!("../../hub-install.ps1"),
    )?;
    fs::write(
        directory.join("THIRD_PARTY_NOTICES.txt"),
        include_bytes!("../../../../THIRD_PARTY_NOTICES.md"),
    )?;
    support::json(
        &directory.join("hub.json"),
        &json!({"format":"ara.hub/1","rehearsal":true,"version":info["version"],"build":info["build"]}),
    )?;
    let files = support::files(&directory)?.into_iter().map(|path| -> Result<Value> {
        Ok(json!({"path":path.strip_prefix(&directory)?.to_string_lossy().replace('\\', "/"),
            "bytes":fs::metadata(&path)?.len(),"blake3":blake3::hash(&fs::read(&path)?).to_string()}))
    }).collect::<Result<Vec<_>>>()?;
    support::json(
        &directory.join("files.json"),
        &json!({"version":info["version"],"rehearsal":true,"files":files}),
    )?;
    let name = format!(
        "ara-hub-{}-windows-x64",
        info["version"].as_str().context("Hub version")?
    );
    let archive = base.join(format!("{name}.zip"));
    crate::archive::zip(&directory, &archive)?;
    let setup_path = base.join(format!("{name}-setup.exe"));
    fs::write(
        &setup_path,
        ara_export_installer::append(&fs::read(setup)?, &fs::read(&archive)?)?,
    )?;
    Ok(Package {
        directory,
        archive,
        setup: setup_path,
        info,
    })
}

fn signed_feed(env: &Environment, new: &Package, key: &Path) -> Result<PathBuf> {
    let version = new.info["version"].as_str().context("new version")?;
    let tag = format!("hub-v{version}");
    let artifacts = env.directory.join("feed");
    let published = artifacts.join("published").join(&tag);
    fs::create_dir_all(&published)?;
    let name = new
        .archive
        .file_name()
        .context("archive filename")?
        .to_string_lossy()
        .into_owned();
    fs::copy(&new.archive, published.join(&name))?;
    let manifest = Manifest { format:"ara.release/1".into(), kind:Kind::Hub, version:version.into(),
        build:Some(serde_json::from_value(new.info["build"].clone())?), channel:Channel::Preview,
        min_hub_version:"0.0.1-alpha".into(), notes:"Private local installed-update replay. Explicit Update now and Restart actions; no Authenticode signing. Never publish rehearsal packages.".into(),
        platform:"windows-x64".into(), assets:vec![Asset { name:name.clone(), size:fs::metadata(&new.archive)?.len(),
            sha256:ara_update::sha256_file(&new.archive)?, url:format!("https://github.com/Ara-Engine/ara/releases/download/{tag}/{name}") }] };
    let bytes = ara_update::canonical_manifest(&manifest)?;
    let seed = fs::read_to_string(key).context("read external throwaway seed file")?;
    let (signature, public) = ara_update::sign(&bytes, &seed)?;
    ara_update::verify(&bytes, signature.as_bytes(), &ara_update::trusted_keys()).context(
        "the release runner and both prebuilt binary sets must trust the supplied disposable key",
    )?;
    fs::write(published.join("release.json"), bytes)?;
    fs::write(published.join("release.json.sig"), signature)?;
    support::json(
        &env.directory.join("signing-public-info.json"),
        &json!({"publicKey":public,"manifest":published.join("release.json")}),
    )?;
    Ok(artifacts)
}

fn step<T>(
    env: &Environment,
    report: &mut Value,
    name: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let started = Instant::now();
    let result = operation();
    report["steps"].as_array_mut().context("replay steps")?.push(json!({"step":name,"passed":result.is_ok(),
        "seconds":started.elapsed().as_secs_f64(),"error":result.as_ref().err().map(|error|format!("{error:#}"))}));
    support::json(&env.directory.join("HUB_REPLAY_RESULTS.json"), report)?;
    result
}

fn wait_ready(child: &mut OwnedProcess, path: &Path) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            ensure!(
                value["restartRequested"] == false,
                "older Hub did not hold at the restart consent gate"
            );
            return Ok(value);
        }
        ensure!(
            child.0.try_wait()?.is_none(),
            "older Hub exited before the ready proof; see older-update.log"
        );
        ensure!(
            Instant::now() < deadline,
            "older Hub timed out before verified-ready; see older-update.log"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_json(path: &Path, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return Ok(value);
        }
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_child(child: &mut OwnedProcess, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.0.try_wait()? {
            ensure!(
                status.success(),
                "older Hub exited {status}; see older-update.log"
            );
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "older Hub did not exit after the restart action"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn pid(value: &Value) -> Result<u32> {
    Ok(value["pid"]
        .as_u64()
        .context("owned replay process PID")?
        .try_into()?)
}

/// Reap only PIDs recorded by this replay, checking executable ownership first.
struct Detached<'a>(&'a Environment);
impl Drop for Detached<'_> {
    fn drop(&mut self) {
        let _ = self.0.stop_hub();
        let paths = [
            "config/rehearsal-updater.json",
            "config/rehearsal-health.json",
            "config/hub/.ara/daemon.json",
        ];
        let ids = paths
            .into_iter()
            .filter_map(|path| fs::read(self.0.directory.join(path)).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter_map(|value| pid(&value).ok())
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if ids.is_empty() {
            return;
        }
        let script = self.0.directory.join("stop-replay-pids.ps1");
        if fs::write(&script, r#"param([string]$Root,[string]$Ids)
$ErrorActionPreference='Stop'
$ownedRoot=[IO.Path]::GetFullPath($Root).TrimEnd('\')+'\'
foreach($ownedId in $Ids.Split(',')) {
  $ownedProcess=Get-CimInstance Win32_Process -Filter ('ProcessId = '+[uint32]$ownedId)
  if($ownedProcess -and $ownedProcess.ExecutablePath -and [IO.Path]::GetFullPath($ownedProcess.ExecutablePath).StartsWith($ownedRoot,[StringComparison]::OrdinalIgnoreCase)) {
    & taskkill.exe /PID $ownedId /T /F 2>$null | Out-Null
  }
}
"#).is_err() { return; }
        let mut command = self.0.command("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .arg(&self.0.directory)
            .arg(ids)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(child) = command.spawn() {
            let mut child = OwnedProcess(child);
            let deadline = Instant::now() + Duration::from_secs(10);
            while child.0.try_wait().is_ok_and(|status| status.is_none())
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}
