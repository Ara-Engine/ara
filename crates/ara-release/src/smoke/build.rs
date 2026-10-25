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
    let session = client
        .call(
            "session.begin",
            json!({"client":"ara-release-smoke","label":"Installed export smoke"}),
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
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    drop(client);
    let until = Instant::now() + Duration::from_secs(10);
    while editor.0.try_wait()?.is_none() {
        ensure!(Instant::now() < until, "export editor did not stop");
        thread::sleep(Duration::from_millis(25));
    }
    let game = output.join("build");
    let log = env.directory.join("exported-game.log");
    super::run(
        env.command(game.join("ara-player.exe"))
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
