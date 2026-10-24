//! Run already-built harnesses with bounded RAM, resource groups and suite deadlines.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{binaries, process, workspace::Metadata};

#[derive(Deserialize)]
struct Tiers {
    slow: Vec<Rule>,
}

#[derive(Clone, Deserialize)]
struct Rule {
    package: String,
    target: String,
    group: String,
    memory_mib: u64,
    timeout_seconds: u64,
    #[serde(default)]
    gpu_packages_only: bool,
}

#[derive(Clone)]
struct Suite {
    package: String,
    target: String,
    kind: String,
    path: PathBuf,
    directory: PathBuf,
    rule: Option<Rule>,
}

#[derive(Serialize)]
pub(crate) struct Execution {
    pub(crate) passed: bool,
    pub(crate) timed_out: bool,
    pub(crate) suites: usize,
    pub(crate) jobs: usize,
    pub(crate) memory_mib: u64,
    pub(crate) seconds: f64,
    results: Vec<SuiteResult>,
}

#[derive(Serialize)]
struct SuiteResult {
    package: String,
    target: String,
    tier: &'static str,
    seconds: f64,
    passed: bool,
    timed_out: bool,
    log: PathBuf,
}

fn matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}

fn rule<'a>(rules: &'a Tiers, package: &str, target: &str) -> Option<&'a Rule> {
    rules.slow.iter().find(|r| {
        matches(&r.package, package)
            && matches(&r.target, target)
            && (!r.gpu_packages_only
                || package.contains("gpu")
                || package.contains("render")
                || package == "ara-pathtrace"
                || package == "ara-samples")
    })
}

fn eligible(suite: &Suite, active: &[Suite], budget: u64) -> bool {
    let cost = |s: &Suite| s.rule.as_ref().map_or(256, |r| r.memory_mib).min(budget);
    active.iter().map(cost).sum::<u64>() + cost(suite) <= budget
        && suite.rule.as_ref().is_none_or(|r| {
            active
                .iter()
                .all(|s| s.rule.as_ref().is_none_or(|a| a.group != r.group))
        })
}

pub(crate) fn run(
    root: &Path,
    logs: &Path,
    metadata: &Metadata,
    selected: &BTreeSet<String>,
    peers: &binaries::Binaries,
    deadline: Option<Instant>,
) -> Result<Execution> {
    let started = Instant::now();
    let tiers: Tiers = toml_edit::de::from_str(include_str!("../test-tiers.toml"))?;
    let mut pending = Vec::new();
    let mut seen = BTreeSet::new();
    for line in fs::read_to_string(logs.join("test-build.json"))?.lines() {
        let Ok(m) = serde_json::from_str::<binaries::Message>(line) else {
            continue;
        };
        if m.reason != "compiler-artifact" || !m.profile.is_some_and(|p| p.test) {
            continue;
        }
        let (Some(path), Some(target), Some(manifest)) = (m.executable, m.target, m.manifest_path)
        else {
            continue;
        };
        let Some(package) = metadata
            .packages
            .iter()
            .find(|p| p.manifest_path == manifest)
        else {
            continue;
        };
        if !selected.contains(&package.name) || !seen.insert(path.clone()) {
            continue;
        }
        pending.push(Suite {
            package: package.name.clone(),
            target: target.name.clone(),
            kind: if target.kind.iter().any(|k| k == "test") {
                "test"
            } else {
                "lib"
            }
            .into(),
            path,
            directory: manifest
                .parent()
                .context("test package directory")?
                .to_owned(),
            rule: rule(&tiers, &package.name, &target.name).cloned(),
        });
    }
    let jobs = std::env::var("ARA_TEST_JOBS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 4);
    let requested_ram = std::env::var("ARA_TEST_RAM_MIB")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(4096)
        .max(256);
    let memory_mib = requested_ram
        .min(crate::platform::available_memory_mib().unwrap_or(requested_ram) / 2)
        .max(256);
    eprintln!(
        "tests: {} prepared harnesses, {jobs} workers, {memory_mib} MiB admission budget; 120s fast / per-rule slow deadlines",
        pending.len()
    );
    let suites = pending.len();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut active = Vec::<(usize, Suite)>::new();
    let mut outcomes = Vec::new();
    std::thread::scope(|scope| -> Result<()> {
        let mut id = 0;
        while !pending.is_empty() || !active.is_empty() {
            while active.len() < jobs {
                let active_suites: Vec<_> = active.iter().map(|(_, s)| s.clone()).collect();
                let Some(index) = pending
                    .iter()
                    .position(|s| eligible(s, &active_suites, memory_mib))
                else {
                    break;
                };
                let suite = pending.remove(index);
                let tx = tx.clone();
                let peers = peers.clone();
                let log = logs.join(format!("suite-{id}.log"));
                let suite_id = id;
                id += 1;
                active.push((suite_id, suite.clone()));
                scope.spawn(move || {
                    let started = Instant::now();
                    let outcome = execute(root, &suite, &log, &peers, deadline);
                    let _ = tx.send((
                        suite_id,
                        suite,
                        log,
                        outcome,
                        started.elapsed().as_secs_f64(),
                    ));
                });
            }
            let (id, suite, log, outcome, seconds) = rx.recv().context("test worker stopped")?;
            active.retain(|(running, _)| *running != id);
            outcomes.push((id, suite, log, outcome, seconds));
        }
        Ok(())
    })?;
    outcomes.sort_by_key(|(id, _, _, _, _)| *id);
    let mut output = File::create(logs.join("tests.log"))?;
    let mut failed = Vec::new();
    let mut timed_out = false;
    let mut results = Vec::new();
    for (_, suite, log, outcome, seconds) in outcomes {
        let passed = outcome.as_ref().is_ok_and(process::Outcome::success);
        results.push(SuiteResult {
            package: suite.package.clone(),
            target: suite.target.clone(),
            tier: if suite.rule.is_some() { "slow" } else { "fast" },
            seconds,
            passed,
            timed_out: matches!(outcome, Ok(process::Outcome::TimedOut)),
            log: log.clone(),
        });
        eprintln!(
            "test {} / {}: {} ({seconds:.2}s)",
            suite.package,
            suite.target,
            if passed { "PASS" } else { "FAIL" }
        );
        writeln!(
            output,
            "Running {} {} ({})",
            suite.package,
            suite.target,
            suite.path.display()
        )?;
        output.write_all(&fs::read(&log).unwrap_or_default())?;
        if !outcome.as_ref().is_ok_and(process::Outcome::success) {
            if matches!(outcome, Ok(process::Outcome::TimedOut)) {
                timed_out = true;
                writeln!(
                    output,
                    "error: suite timeout: {} {}",
                    suite.package, suite.target
                )?;
            } else if let Err(error) = outcome {
                writeln!(output, "error: test launch: {error:#}")?;
            } else if let Ok(process::Outcome::Exited(status)) = outcome
                && status.code() != Some(101)
            {
                writeln!(output, "error: abnormal test exit: {status}")?;
            }
            let target = if suite.kind == "test" {
                format!("-p {} --test {}", suite.package, suite.target)
            } else {
                format!("-p {} --lib", suite.package)
            };
            writeln!(output, "error: test failed, to rerun pass `{target}`")?;
            failed.push(target);
        }
    }
    if !failed.is_empty() {
        writeln!(output, "error: {} targets failed:", failed.len())?;
        for target in &failed {
            writeln!(output, "    `{target}`")?;
        }
    }
    let execution = Execution {
        passed: failed.is_empty(),
        timed_out,
        suites,
        jobs,
        memory_mib,
        seconds: started.elapsed().as_secs_f64(),
        results,
    };
    fs::write(
        logs.join("test-execution.json"),
        serde_json::to_vec_pretty(&execution)?,
    )?;
    Ok(execution)
}

fn execute(
    root: &Path,
    suite: &Suite,
    log: &Path,
    peers: &binaries::Binaries,
    deadline: Option<Instant>,
) -> Result<process::Outcome> {
    let file = File::create(log)?;
    let mut command = Command::new(&suite.path);
    let search_var = if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut search = vec![
        suite
            .path
            .parent()
            .context("test binary directory")?
            .to_owned(),
    ];
    if let Some(profile) = search[0].parent() {
        search.push(profile.to_owned());
    }
    search.extend(std::env::split_paths(
        &std::env::var_os(search_var).unwrap_or_default(),
    ));
    command.env(search_var, std::env::join_paths(search)?);
    command
        .current_dir(&suite.directory)
        .env(
            "RUST_TEST_THREADS",
            if suite.rule.is_some() { "1" } else { "2" },
        )
        .env(
            "ARA_CONFIG_DIR",
            root.join("target/ara-dev/test-profiles").join(format!(
                "{}-{}-{}",
                std::process::id(),
                suite.package,
                suite.target
            )),
        )
        .env(
            "ARA_CRASH_DIR",
            root.join("target/ara-dev/test-crashes").join(format!(
                "{}-{}-{}",
                std::process::id(),
                suite.package,
                suite.target
            )),
        )
        .stdout(Stdio::from(file.try_clone()?))
        .stderr(Stdio::from(file));
    for (name, path) in peers {
        command.env(format!("CARGO_BIN_EXE_{name}"), path);
    }
    let timeout = std::env::var("ARA_TEST_TIMEOUT_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| suite.rule.as_ref().map_or(120, |r| r.timeout_seconds));
    ensure!(timeout > 0, "test timeout must be positive");
    let own = Instant::now() + Duration::from_secs(timeout);
    process::run(
        &mut command,
        Some(deadline.map_or(own, |gate| gate.min(own))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_resources_serialize_and_ram_limits_parallel_admission() {
        let tiers: Tiers = toml_edit::de::from_str(include_str!("../test-tiers.toml")).unwrap();
        let gpu = Suite {
            package: "ara-player".into(),
            target: "scenario".into(),
            kind: "test".into(),
            path: "test".into(),
            directory: ".".into(),
            rule: rule(&tiers, "ara-player", "scenario").cloned(),
        };
        let cpu = Suite {
            rule: None,
            ..gpu.clone()
        };
        assert!(!eligible(&gpu, std::slice::from_ref(&gpu), 4096));
        assert!(eligible(&cpu, std::slice::from_ref(&gpu), 4096));
        assert!(!eligible(&gpu, std::slice::from_ref(&cpu), 2048));
        assert!(eligible(&gpu, &[], 512));
        assert!(rule(&tiers, "ara-render-fidelity", "headless").is_some());
        assert!(rule(&tiers, "ara-schema", "ara_schema").is_none());
    }
}
