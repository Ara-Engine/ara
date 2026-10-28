//! The lab-admitted subprocess requests one build, then exits with staged metadata.
use crate::lab::build::{ReloadStrategy, WarmRestart};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn run(root: &Path, target: &Path, source: &Path) -> Result<bool> {
    // The initial fat build finishes before the lab launches a GPU-admitted player.
    let prepared = super::source::wait(source)?;
    let base = crate::lab::directory(root);
    let endpoint = super::read(&base.join("endpoint.json"));
    let mut stream = TcpStream::connect(endpoint["address"].as_str().context("lab endpoint")?)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    writeln!(
        stream,
        "{}",
        json!({"token":endpoint["token"],"tool":"lab_status","arguments":{}})
    )?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let status: Value = serde_json::from_str(&line)?;
    let hosts: Vec<_> = status["result"]["sessions"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(|track| {
            let directory = PathBuf::from(track["directory"].as_str()?);
            let host = super::read(&directory.join("control/rust/host.json"));
            (host["owner_pid"] == track["pid"]
                && host["pid"].as_u64().is_some()
                && host["contract"]
                    .as_str()
                    .is_some_and(|path| Path::new(path) == prepared.contract))
            .then_some(host)
        })
        .collect();
    if hosts.len() == 1 {
        let mut host = hosts.into_iter().next().context("one hotpatch host")?;
        let contract = ara_hotpatch::Contract::read(Path::new(
            host["contract"].as_str().context("host baseline")?,
        ))?;
        if let Ok(revision) = contract.revision() {
            let control = PathBuf::from(host["control"].as_str().context("host control")?);
            let directory = control.join("rust");
            host["revision"] = json!(revision);
            let current = super::read(&directory.join("applied.json"));
            if current["revision"] != revision {
                // Retrying an expired/rejected publication of identical sources
                // must request a new build instead of reusing an old failure.
                let rejected = directory.join("rejected.json");
                if rejected.exists() {
                    fs::remove_file(rejected)?;
                }
                fs::write(
                    directory.join("build"),
                    format!("{revision}:{}", ara_hotpatch::timestamp_us()),
                )?;
                let began = Instant::now();
                loop {
                    let staged = super::read(&directory.join("staged.json"));
                    if staged["revision"] == revision && staged["pid"] == host["pid"] {
                        break;
                    }
                    let rejected = super::read(&directory.join("rejected.json"));
                    ensure!(
                        rejected["revision"] != revision,
                        "Rust patch compile rejected: {}",
                        rejected["reason"]
                    );
                    ensure!(
                        began.elapsed() < Duration::from_secs(180),
                        "Rust patch staging timed out; last good retained"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let batches = base.join("hotpatch-batches");
            fs::create_dir_all(&batches)?;
            let batch = batches.join(format!("{}.json", ara_hotpatch::timestamp_us()));
            fs::write(&batch, serde_json::to_vec(&host)?)?;
            println!("{}", json!({"ara_hotpatch_batch":batch}));
            return Ok(true);
        }
        eprintln!(
            "ARA7602: source/layout change needs a new lab baseline; rebuilding with warm restart"
        );
    }
    let mut command = WarmRestart.command(root, target);
    command.env("CARGO_BUILD_JOBS", super::super::tool::build_jobs());
    Ok(crate::process::run(&mut command, None)?.success())
}
