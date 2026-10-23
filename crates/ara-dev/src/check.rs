//! One report combines cheap hygiene checks with logged Cargo gates.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{git, hygiene, workspace::Metadata};

#[derive(Serialize)]
struct Row {
    check: String,
    passed: bool,
    seconds: f64,
    detail: String,
}

#[derive(Serialize)]
struct Report {
    passed: bool,
    base: String,
    affected: BTreeSet<String>,
    checks: Vec<Row>,
}

pub(crate) struct Logs {
    pub(crate) directory: PathBuf,
    rows: Vec<Row>,
}

impl Logs {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let directory = root.join("target/ara-dev").join(format!(
            "run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));
        fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            rows: Vec::new(),
        })
    }

    pub(crate) fn command(&mut self, root: &Path, name: &str, args: &[String]) -> Result<bool> {
        let log = self.directory.join(format!("{name}.log"));
        let file = File::create(&log)?;
        eprintln!("{name}: cargo {} (log: {})", args.join(" "), log.display());
        let start = Instant::now();
        let status = Command::new("cargo")
            .current_dir(root)
            .args(args)
            .env("CARGO_BUILD_JOBS", "2")
            .stdout(Stdio::from(file.try_clone()?))
            .stderr(Stdio::from(file))
            .status();
        let passed = status.as_ref().is_ok_and(|s| s.success());
        let detail = match status {
            Ok(s) => format!("{s}; {}", log.display()),
            Err(e) => format!("{e}; {}", log.display()),
        };
        self.rows.push(Row {
            check: name.into(),
            passed,
            seconds: start.elapsed().as_secs_f64(),
            detail,
        });
        Ok(passed)
    }

    fn findings(&mut self, name: &str, findings: Vec<String>) {
        self.rows.push(Row {
            check: name.into(),
            passed: findings.is_empty(),
            seconds: 0.0,
            detail: if findings.is_empty() {
                "ok".into()
            } else {
                findings.join("\n")
            },
        });
    }
}

pub(crate) fn selection(command: &str, affected: &BTreeSet<String>, full: bool) -> Vec<String> {
    let mut args = vec![command.into()];
    if full {
        args.push("--workspace".into());
    } else {
        for package in affected {
            args.extend(["-p".into(), package.clone()]);
        }
    }
    args
}

pub(crate) fn run(root: &Path, base: &str, json: bool, full: bool) -> Result<bool> {
    // Capture before metadata, too: Cargo itself must not silently rewrite Cargo.lock.
    let before = git::TreeState::read(root)?;
    let base = git::text(root, ["merge-base", "HEAD", base])
        .context("resolve --base (use an existing local ref)")?;
    let changed = git::changed(root, &base)?;
    let metadata = Metadata::read(root)?;
    eprintln!("Cargo target: {}", metadata.target_directory.display());
    let affected = metadata.affected(root, &changed);
    if affected.contains("ara-dev") || full {
        crate::platform::release_executable_path(&metadata.target_directory)?;
    }
    let files = git::files(root)?;
    let mut logs = Logs::new(root)?;
    logs.findings("exact pins", hygiene::manifests(root, &changed)?);
    logs.findings("AGENTS.md", hygiene::agents(root, &files)?);
    logs.findings(
        "importer versions",
        hygiene::importer_versions(root, &files)?,
    );
    let fmt = logs.command(
        root,
        "fmt",
        &["fmt".into(), "--all".into(), "--".into(), "--check".into()],
    )?;
    if fmt && (full || !affected.is_empty()) {
        let whole_workspace = full || affected.len() == metadata.packages.len();
        let mut clippy = selection("clippy", &affected, whole_workspace);
        clippy.extend([
            "--all-targets".into(),
            "--locked".into(),
            "--".into(),
            "-D".into(),
            "warnings".into(),
        ]);
        logs.command(root, "clippy", &clippy)?;
        let mut tests = selection("test", &affected, whole_workspace);
        tests.extend(["--locked".into(), "--no-fail-fast".into()]);
        logs.command(root, "tests", &tests)?;
    }
    // Coverage scans sources and C#, so always run it when code or catalog inputs change.
    let codes = changed
        .iter()
        .any(|p| p.starts_with("crates") || p.starts_with("dotnet"));
    if codes {
        logs.command(
            root,
            "diagnostic-coverage",
            &["test", "-p", "ara-diag", "--test", "coverage", "--locked"].map(String::from),
        )?;
    }
    let after = git::TreeState::read(root)?;
    logs.findings(
        "unchanged tree",
        before
            .verify(&after)
            .err()
            .map(|e| e.to_string())
            .into_iter()
            .collect(),
    );
    let mut snapshots: Vec<_> = git::files(root)?
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".snap.new"))
        .map(|p| p.display().to_string())
        .collect();
    // Include ignored new snapshots too; insta can inherit ignore rules.
    snapshots.extend(
        git::paths(&git::bytes(
            root,
            [
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
                "crates/**/*.snap.new",
            ],
        )?)?
        .into_iter()
        .map(|p| p.display().to_string()),
    );
    snapshots.sort();
    snapshots.dedup();
    logs.findings("pending snapshots", snapshots);
    let report = Report {
        passed: logs.rows.iter().all(|r| r.passed),
        base,
        affected,
        checks: logs.rows,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Affected ({}): {}",
            report.affected.len(),
            report
                .affected
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        if report.affected.len() > 8 {
            println!(
                "  ... {} more (use --json for the complete set)",
                report.affected.len() - 8
            );
        }
        println!("{:<22} {:<6} {:>8}  DETAIL", "CHECK", "RESULT", "SECONDS");
        for row in &report.checks {
            println!(
                "{:<22} {:<6} {:>8.1}  {}",
                row.check,
                if row.passed { "PASS" } else { "FAIL" },
                row.seconds,
                row.detail
            );
        }
    }
    Ok(report.passed)
}
