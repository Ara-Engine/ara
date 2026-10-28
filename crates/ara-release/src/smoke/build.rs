//! Build Current Profile through the installed editor, then execute its game.
use super::*;

const TICKS: u64 = 8;

pub(super) fn run(env: &Environment, project: &Path) -> Result<()> {
    let pdb = env.installed.join("ara_player.pdb");
    let pdb_hash = hash(&pdb).context("installed runtime-template PDB is missing")?;
    let inventory: Value = serde_json::from_slice(&fs::read(env.installed.join("files.json"))?)?;
    ensure!(
        inventory["files"].as_array().is_some_and(|files| files
            .iter()
            .any(|file| file["path"] == "ara_player.pdb" && file["blake3"] == pdb_hash)),
        "installed player PDB differs from the package inventory"
    );
    let log = fs::File::create(env.directory.join("export-editor.log"))?;
    let mut editor = OwnedProcess(
        env.command(env.installed.join("ara-editor.exe"))
            .arg(project)
            .args(["--frames", "1000000", "--size", "640x480", "--output"])
            .arg(env.directory.join("export-editor"))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?,
    );
    let until = Instant::now() + Duration::from_secs(60);
    let mut client = loop {
        ensure!(
            editor.0.try_wait()?.is_none(),
            "installed editor exited before export; see export-editor.log"
        );
        if let Some(client) = ara_rpc::Client::for_project(project) {
            break client;
        }
        ensure!(
            Instant::now() < until,
            "installed editor startup timed out; see export-editor.log"
        );
        thread::sleep(Duration::from_millis(100));
    };
    // This smoke drives the human File -> Build flow and owns the editor it
    // gracefully shuts down. An omitted author defaults to an agent session.
    let session = client
        .call(
            "session.begin",
            json!({"client":"ara-release-smoke","author":"human","label":"Installed export smoke"}),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let queued = client
        .call(
            "editor.command",
            json!({"session":session["session"],"id":"build.current"}),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    ensure!(
        queued["queued"] == true,
        "Build Current Profile was not queued: {queued}"
    );
    let until = Instant::now() + Duration::from_secs(900);
    let output = loop {
        ensure!(
            editor.0.try_wait()?.is_none(),
            "installed editor exited during export; see export-editor.log"
        );
        let diagnostics = client
            .call(
                "diag.query",
                json!({"session":session["session"],"codes":["ARA4505"],"group":false}),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let entries = diagnostics["entries"]
            .as_array()
            .context("build diagnostic entries")?;
        ensure!(
            !entries
                .iter()
                .any(|entry| entry["diagnostic"]["severity"] == "error"),
            "Build Current Profile failed: {diagnostics}"
        );
        // The editor acknowledges completion only after copying every export
        // artifact. A report file appearing mid-copy is not readiness.
        let finished = entries.iter().any(|entry| {
            entry["diagnostic"]["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("Built "))
        });
        let builds = project.join("builds");
        if finished && builds.is_dir() {
            let mut ready = None;
            for entry in fs::read_dir(&builds)? {
                let output = entry?.path();
                if output.join("report.json").is_file() {
                    let report: Value =
                        serde_json::from_slice(&fs::read(output.join("report.json"))?)?;
                    ensure!(
                        report["verify"]["passed"] == true
                            && report["verify"]["runtime_executed"] == true,
                        "export did not execute the runtime: {report}"
                    );
                    ready = Some(output);
                    break;
                }
            }
            if let Some(output) = ready {
                break output;
            }
        }
        ensure!(
            Instant::now() < until,
            "Build Current Profile timed out; see export-editor.log and the project's .ara/export logs"
        );
        thread::sleep(Duration::from_millis(250));
    };
    let cached_pdb = project.join(".ara/export/cache/templates").join(&pdb_hash);
    ensure!(
        hash(&cached_pdb)? == pdb_hash,
        "exporter's hash-pinned PDB does not match the installed player PDB"
    );
    client
        .call("daemon.shutdown", json!({"session":session["session"]}))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("stop the smoke's export editor authority through its human session")?;
    drop(client);
    let until = Instant::now() + Duration::from_secs(10);
    while editor.0.try_wait()?.is_none() {
        ensure!(Instant::now() < until, "export editor did not stop");
        thread::sleep(Duration::from_millis(25));
    }
    let game = output.join("build");
    let distribution: Value = serde_json::from_slice(&fs::read(output.join("distribution.json"))?)?;
    let entry = distribution["entryPoint"]
        .as_str()
        .context("distribution entry point")?;
    validate_shipped_layout(&game, entry)?;
    ensure!(
        output.join("report/verify.png").is_file(),
        "export verification capture is missing from the private report directory"
    );
    let log = env.directory.join("exported-game.log");
    super::run(
        env.command(game.join(entry))
            .current_dir(&game)
            .arg("--root")
            .arg(&game)
            .args(["--headless", "--ticks", &TICKS.to_string(), "--capture"])
            .arg(env.directory.join("exported-game.png")),
        &log,
    )?;
    let result = runtime_result(&fs::read_to_string(log)?)?;
    support::json(&env.directory.join("exported-game-result.json"), &result)?;
    Ok(())
}

/// The store upload root contains only the branded launcher, its data and notices.
fn validate_shipped_layout(game: &Path, entry: &str) -> Result<()> {
    ensure!(
        Path::new(entry).components().count() == 1
            && Path::new(entry)
                .extension()
                .is_some_and(|extension| extension == "exe")
            && entry != "ara-player.exe",
        "export must use the game's Windows executable name: {entry}"
    );
    for file in fs::read_dir(game)? {
        let file = file?;
        let name = file.file_name();
        ensure!(
            name == entry
                || name == "data"
                || name == "THIRD_PARTY_NOTICES.txt"
                || name == "NOTICES.txt"
                || name == "LICENSE.txt",
            "unexpected file in shipped folder: {}",
            file.path().display()
        );
    }
    ensure!(
        game.join(entry).is_file()
            && game.join("data").is_dir()
            && game.join("THIRD_PARTY_NOTICES.txt").is_file(),
        "shipped game is missing its launcher, data folder or notices"
    );
    fn private_files(path: &Path) -> Result<()> {
        for file in fs::read_dir(path)? {
            let file = file?;
            let name = file.file_name().to_string_lossy().to_lowercase();
            ensure!(
                !name.starts_with("verify.")
                    && name != "logs"
                    && name != "symbols"
                    && !name.ends_with(".pdb")
                    && !name.ends_with(".render_ids.bin"),
                "private export artifact leaked into shipped game: {}",
                file.path().display()
            );
            if file.file_type()?.is_dir() {
                private_files(&file.path())?;
            }
        }
        Ok(())
    }
    private_files(game)
}

fn hash(path: &Path) -> Result<String> {
    let mut digest = blake3::Hasher::new();
    digest.update_reader(fs::File::open(path)?)?;
    Ok(digest.finalize().to_string())
}

fn runtime_result(output: &str) -> Result<Value> {
    let result = output
        .lines()
        .find_map(|line| line.strip_prefix("ARA_RESULT "))
        .context("exported game emitted no ARA_RESULT")?;
    let result: Value = serde_json::from_str(result)?;
    ensure!(
        result["verified"] == true
            && result["tick"].as_u64() == Some(TICKS)
            && result["template_contract"] == "ara.player.runtime/1",
        "exported game failed its runtime contract or did not execute {TICKS} ticks: {result}"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_layout_rejects_private_artifacts_and_an_unbranded_launcher() -> Result<()> {
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release-layout-tests");
        fs::create_dir_all(&parent)?;
        let root = tempfile::tempdir_in(parent)?;
        fs::create_dir(root.path().join("data"))?;
        fs::write(root.path().join("Outpost.exe"), b"launcher")?;
        fs::write(root.path().join("THIRD_PARTY_NOTICES.txt"), b"notices")?;
        validate_shipped_layout(root.path(), "Outpost.exe")?;
        assert!(validate_shipped_layout(root.path(), "ara-player.exe").is_err());
        assert!(validate_shipped_layout(root.path(), "../Outpost.exe").is_err());
        for name in [
            "verify.png",
            "verify.json",
            "verify.legend.json",
            "verify.render_ids.bin",
            "ara_player.pdb",
        ] {
            let artifact = root.path().join("data").join(name);
            fs::write(&artifact, b"private")?;
            assert!(
                validate_shipped_layout(root.path(), "Outpost.exe").is_err(),
                "{name}"
            );
            fs::remove_file(artifact)?;
        }
        Ok(())
    }

    #[test]
    fn smoke_requires_a_verified_runtime_result_with_the_requested_ticks() {
        let valid =
            json!({"verified":true,"tick":TICKS,"template_contract":"ara.player.runtime/1"});
        assert!(runtime_result(&format!("log line\nARA_RESULT {valid}\n")).is_ok());
        assert!(runtime_result("process exited successfully").is_err());
        for (key, value) in [
            ("verified", json!(false)),
            ("tick", json!(0)),
            ("template_contract", json!("ara.player.conformance/1")),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(
                runtime_result(&format!("ARA_RESULT {invalid}")).is_err(),
                "{key}"
            );
        }
    }
}
